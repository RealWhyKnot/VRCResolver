use std::sync::Arc;
use std::time::SystemTime;

pub type Now = Arc<dyn Fn() -> SystemTime + Send + Sync>;

pub fn system() -> Now {
    Arc::new(SystemTime::now)
}
