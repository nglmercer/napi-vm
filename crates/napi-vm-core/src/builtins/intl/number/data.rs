use super::*;
use std::sync::{Arc, Mutex, OnceLock};
include!("../cldr/number-pattern-index.rs");
const DATA: &[u8] = include_bytes!("../cldr/number-patterns.bin");
pub(super) fn locale(name: &str) -> Result<Arc<Json>, VmErr> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<&'static str, Arc<Json>>>> =
        OnceLock::new();
    let mut parsed: Locale = name.parse().map_err(|_| range())?;
    let mut resolved = parsed.id.to_string();
    if INDEX
        .binary_search_by_key(&resolved.as_str(), |r| r.0)
        .is_err()
    {
        icu_locale::LocaleExpander::new_extended().maximize(&mut parsed.id);
        resolved = parsed.id.to_string();
    }
    let mut candidate = resolved.as_str();
    let entry = loop {
        if let Ok(index) = INDEX.binary_search_by_key(&candidate, |r| r.0) {
            break INDEX[index];
        }
        candidate = candidate.rsplit_once('-').map_or("en", |p| p.0);
    };
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    if let Some(data) = cache.get(entry.0) {
        return Ok(data.clone());
    }
    let decoder = flate2::read::GzDecoder::new(&DATA[entry.1..entry.2]);
    let data: Arc<Json> = Arc::new(
        serde_json::from_reader(decoder)
            .map_err(|e| VmErr::Msg(format!("Invalid compiled CLDR data: {e}")))?,
    );
    cache.insert(entry.0, data.clone());
    Ok(data)
}
pub(super) fn digits(name: &str) -> Option<&'static str> {
    NUMBERING_SYSTEMS
        .binary_search_by_key(&name, |r| r.0)
        .ok()
        .map(|i| NUMBERING_SYSTEMS[i].1)
}
pub(super) fn unit_valid(unit: &str) -> bool {
    let parts = unit.split("-per-").collect::<Vec<_>>();
    parts.len() <= 2 && parts.iter().all(|p| UNITS.contains(p))
}
