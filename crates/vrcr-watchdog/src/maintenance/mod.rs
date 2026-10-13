pub mod codec;
pub mod patch;
pub mod shutdown;
pub mod update;

pub fn describe_parent_process() -> Option<String> {
    vrcr_shared::win::describe_parent(&vrcr_shared::win::list_processes(), std::process::id())
}
