use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, GetLastError, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Cryptography::{
    CERT_CONTEXT, CERT_STORE_ADD_USE_EXISTING, CERT_STORE_OPEN_EXISTING_FLAG,
    CERT_STORE_PROV_SYSTEM_W, CERT_STORE_READONLY_FLAG, CERT_SYSTEM_STORE_LOCAL_MACHINE,
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CertAddEncodedCertificateToStore,
    CertCloseStore, CertDeleteCertificateFromStore, CertDuplicateCertificateContext,
    CertEnumCertificatesInStore, CertOpenStore, CryptProtectData, CryptUnprotectData, HCERTSTORE,
    PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};

use super::{Elevated, MachineStore};

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

struct Store(HCERTSTORE);

impl Store {
    fn open(which: MachineStore, writable: bool) -> io::Result<Store> {
        let name = wide(OsStr::new(which.name()));
        let mut flags = CERT_SYSTEM_STORE_LOCAL_MACHINE | CERT_STORE_OPEN_EXISTING_FLAG;
        if !writable {
            flags |= CERT_STORE_READONLY_FLAG;
        }
        let handle =
            // SAFETY: name is a live NUL-terminated store name matching the system-store provider.
            unsafe { CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, flags, name.as_ptr().cast()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Store(handle))
    }

    fn each(&self, mut visit: impl FnMut(*const CERT_CONTEXT, &[u8])) {
        let mut ctx: *const CERT_CONTEXT = std::ptr::null();
        loop {
            // SAFETY: self owns the store; ctx is null or the previous enumeration context consumed here.
            ctx = unsafe { CertEnumCertificatesInStore(self.0, ctx) };
            if ctx.is_null() {
                return;
            }
            // SAFETY: Successful enumeration returns a live context whose encoded buffer has cbCertEncoded bytes.
            let der = unsafe {
                let c = &*ctx;
                std::slice::from_raw_parts(
                    c.pbCertEncoded,
                    usize::try_from(c.cbCertEncoded)
                        .expect("Windows pointer width holds a certificate length"),
                )
            };
            visit(ctx, der);
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: self uniquely owns this successfully opened certificate-store handle.
        unsafe { CertCloseStore(self.0, 0) };
    }
}

pub fn machine_store_certs(which: MachineStore) -> io::Result<Vec<Vec<u8>>> {
    let store = Store::open(which, false)?;
    let mut out = Vec::new();
    store.each(|_, der| out.push(der.to_vec()));
    Ok(out)
}

pub fn machine_store_add(which: MachineStore, der: &[u8]) -> io::Result<()> {
    let store = Store::open(which, true)?;
    let len = u32::try_from(der.len())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    // SAFETY: store is open and der contains exactly len readable bytes for this call.
    let ok = unsafe {
        CertAddEncodedCertificateToStore(
            store.0,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            der.as_ptr(),
            len,
            CERT_STORE_ADD_USE_EXISTING,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn machine_store_remove(
    which: MachineStore,
    remove: impl Fn(&[u8]) -> bool,
) -> io::Result<usize> {
    let store = Store::open(which, true)?;
    let mut removed = 0;
    let mut failure: Option<io::Error> = None;
    store.each(|ctx, der| {
        if !remove(der) {
            return;
        }
        // SAFETY: ctx is the live enumeration context supplied by Store::each.
        let dup = unsafe { CertDuplicateCertificateContext(ctx) };
        // SAFETY: dup is the duplicated certificate context; this call consumes that reference.
        if unsafe { CertDeleteCertificateFromStore(dup) } == 0 {
            failure.get_or_insert_with(io::Error::last_os_error);
        } else {
            removed += 1;
        }
    });
    match failure {
        Some(e) => Err(e),
        None => Ok(removed),
    }
}

fn blob(data: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
        pbData: data.as_ptr().cast_mut(),
    })
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: Only successful DPAPI calls pass their allocated output buffer and byte count to take.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            out.pbData,
            usize::try_from(out.cbData)
                .expect("Windows pointer width holds the DPAPI output length"),
        )
    }
    .to_vec();
    // SAFETY: The DPAPI output buffer was allocated with LocalAlloc and is freed once after copying.
    unsafe { LocalFree(out.pbData.cast()) };
    bytes
}

pub fn dpapi_protect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = blob(data)?;
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: input borrows live data, out is writable, and all optional pointers are null.
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(take(out))
}

pub fn dpapi_unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = blob(data)?;
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: input borrows live data, out is writable, and all optional pointers are null.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(take(out))
}

pub fn run_elevated(exe: &Path, args: &str, timeout: Duration) -> Elevated {
    let verb = wide(OsStr::new("runas"));
    let file = wide(exe.as_os_str());
    let params = wide(OsStr::new(args));
    // SAFETY: SHELLEXECUTEINFOW consists of integers and nullable pointers, all valid when zeroed.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or(u32::MAX);
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = 0;
    // SAFETY: info has its size set and all referenced NUL-terminated strings outlive the synchronous call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // SAFETY: GetLastError reads the error from the preceding call on this thread.
        let code = unsafe { GetLastError() };
        if code == ERROR_CANCELLED {
            return Elevated::Declined;
        }
        return Elevated::Failed(
            io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX)).to_string(),
        );
    }
    if info.hProcess.is_null() {
        return Elevated::Exited(0);
    }
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: info.hProcess is the non-null process handle returned with SEE_MASK_NOCLOSEPROCESS.
    let waited = unsafe { WaitForSingleObject(info.hProcess, ms) };
    let result = if waited == WAIT_OBJECT_0 {
        let mut code = 0u32;
        // SAFETY: info.hProcess is still open and code is a live writable u32.
        if unsafe { GetExitCodeProcess(info.hProcess, &mut code) } == 0 {
            Elevated::Failed(io::Error::last_os_error().to_string())
        } else {
            Elevated::Exited(code)
        }
    } else if waited == WAIT_TIMEOUT {
        Elevated::TimedOut
    } else {
        Elevated::Failed(io::Error::last_os_error().to_string())
    };
    // SAFETY: This is the owned process handle returned by ShellExecuteExW and its only close.
    unsafe { CloseHandle(info.hProcess) };
    result
}
