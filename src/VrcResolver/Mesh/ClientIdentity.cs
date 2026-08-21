using VrcResolver.Shared;

namespace VrcResolver;

// Per-install stable client identity. Persisted at
//   %LOCALAPPDATA%Low\vrcresolver\client_id.txt
// (next to clean_exit.flag, codec-state.json, v3_welcome_cache.json — same
// LocalLow state-root convention from project_locallow_state_layout.md).
//
// Used as the client_id field on:
//   * v3 ClientHelloFrame (server keys server-side telemetry on this).
//   * playback_feedback frames (correlates a returning watchdog to its
//     prior failure reports without socket-address matching).
//
// Pre-fix this was a fresh Guid.NewGuid() in MeshClient's field
// initializer — fresh per process launch. The watchdog launches once per
// VRChat session, so server-side analytics keying on client_id couldn't
// recognise a returning user across launches. Persisting the GUID once
// and reading it on subsequent launches makes the same install present
// the same identity for its full lifetime.
//
// Identity rotates only on:
//   * Uninstall + reinstall (the file lives under the LocalLow state
//     root which the uninstaller wipes).
//   * Manual deletion of the client_id.txt file.
//   * A fresh install on a different user account (different LocalLow
//     root).
//
// Best-effort throughout: read failures (permissions, corrupt content),
// write failures (disk full, file locked), or directory-create failures
// all degrade to a fresh in-memory GUID for this run rather than
// throwing — server-side analytics tolerate "client we've never seen"
// gracefully.
internal static class ClientIdentity
{
    internal const string FileName = "client_id.txt";

    // Defensive cap on the file's read size. Legitimate file is 32 bytes
    // (one N-format GUID) plus at most a trailing CRLF (34 bytes). 256
    // bytes is ~7× that — generous enough to tolerate hand-edits with
    // extra whitespace, tight enough that a corrupt or hostile multi-GB
    // file can't induce a File.ReadAllText alloc before the catch fires.
    internal const long MaxClientIdFileBytes = 256;

    public static string LoadOrCreate() => LoadOrCreate(DefaultPath());

    // Test-only overload: caller supplies the file path so unit tests can
    // exercise the load/create/corrupt-file paths in a temp dir without
    // touching the real LocalLow state root.
    internal static string LoadOrCreate(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                var info = new FileInfo(path);
                if (info.Length > MaxClientIdFileBytes)
                {
                    // File is far larger than any legitimate client_id —
                    // corrupt, hostile, or unrelated file dropped at our
                    // path. Skip the read, fall through to regenerate +
                    // overwrite. Don't try to ReadAllText: a 1 GB file
                    // would alloc 1 GB before the (caught) OOM.
                    // No file-only log here — ClientIdentity has no
                    // Logger dependency by design (it runs at MeshClient
                    // field-init time, before Logger.Install). Falling
                    // through to write the fresh value overwrites the
                    // pathological file in place.
                }
                else
                {
                    string content = File.ReadAllText(path).Trim();
                    if (Guid.TryParseExact(content, "N", out var g))
                        return g.ToString("N");
                    // File present but contents not a 32-hex-digit GUID —
                    // treat as corrupt, regenerate. Don't throw.
                }
            }
        }
        catch { /* fall through to generate + write */ }

        string fresh = Guid.NewGuid().ToString("N");
        string tmp = path + ".new";
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.WriteAllText(tmp, fresh);
            File.Move(tmp, path, overwrite: true);
        }
        catch
        {
            // In-memory only this run. Next launch will retry the file
            // and either succeed (if the transient cause cleared) or
            // generate ANOTHER fresh GUID. Server sees two short-lived
            // identities — same as pre-persistence behaviour, no worse.
            // Clean up any tmp residue so a partial write doesn't
            // accumulate orphan .new files on disk.
            try { if (File.Exists(tmp)) File.Delete(tmp); } catch { /* best-effort */ }
        }
        return fresh;
    }

    private static string DefaultPath() =>
        Path.Combine(AppPaths.StateRoot(), FileName);
}
