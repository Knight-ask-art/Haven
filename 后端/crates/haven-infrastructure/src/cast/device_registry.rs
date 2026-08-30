use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use haven_application::wire::CastProtocolDto;

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub ip: String,
    pub protocol: CastProtocolDto,
    pub control_url: Option<String>,
    pub location: Option<String>,
}

static REGISTRY: OnceLock<Arc<Mutex<HashMap<String, DeviceInfo>>>> = OnceLock::new();

fn registry() -> Arc<Mutex<HashMap<String, DeviceInfo>>> {
    REGISTRY
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

pub fn insert(device_id: String, info: DeviceInfo) {
    let reg = registry();
    let mut map = reg.lock().unwrap_or_else(|e| e.into_inner());
    map.insert(device_id, info);
}

pub fn get(device_id: &str) -> Option<DeviceInfo> {
    let reg = registry();
    let map = reg.lock().unwrap_or_else(|e| e.into_inner());
    map.get(device_id).cloned()
}

pub fn remove(device_id: &str) {
    let reg = registry();
    let mut map = reg.lock().unwrap_or_else(|e| e.into_inner());
    map.remove(device_id);
}
