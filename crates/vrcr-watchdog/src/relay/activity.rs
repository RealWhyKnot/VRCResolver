use std::collections::HashMap;
use std::sync::Mutex;

use url::Url;
use vrcr_shared::trust_gateway::GATEWAY_HOST;

const MAX_TRACKED: usize = 512;

#[derive(Default)]
pub struct RelayActivity {
    seen: Mutex<Seen>,
}

#[derive(Default)]
struct Seen {
    order: u64,
    plays: HashMap<String, u64>,
}

impl RelayActivity {
    pub fn new() -> RelayActivity {
        RelayActivity::default()
    }

    pub fn record(&self, path: &str) {
        let Some(id) = play_id(path) else {
            return;
        };
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.order += 1;
        let order = seen.order;
        seen.plays.insert(id.to_owned(), order);
        if seen.plays.len() > MAX_TRACKED {
            let mut by_age: Vec<(u64, String)> = seen.plays.iter().map(|(k, v)| (*v, k.clone())).collect();
            by_age.sort_unstable();
            let drop = seen.plays.len() - MAX_TRACKED / 2;
            for (_, key) in by_age.into_iter().take(drop) {
                seen.plays.remove(&key);
            }
        }
    }

    pub fn wedged_port(&self, opened_url: &str) -> Option<u16> {
        let url = Url::parse(opened_url).ok()?;
        if !url.host_str()?.eq_ignore_ascii_case(GATEWAY_HOST) {
            return None;
        }
        let id = play_id(url.path())?;
        let port = url.port_or_known_default()?;
        let seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        (!seen.plays.contains_key(id)).then_some(port)
    }

    pub fn tracked(&self) -> usize {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).plays.len()
    }
}

pub fn play_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/play/")?;
    let id = rest.split('/').next().unwrap_or_default();
    (!id.is_empty()).then_some(id)
}
