//! Build runtime time-zone identifier tables from a pinned IANA tzdata release.
//!
//! Usage: `tz-data <cache-dir> <output-dir>`. Downloads the tzdata tarball into
//! the cache when missing, then writes `timezone-names.rs` into the output
//! directory. No Test262 data is consulted.

use napi_vm_generators::{Member, basename, download_once, json_string, read_tgz, write_output};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSION: &str = "2025b";

/// Country codes for Link names that appear in neither zone.tab nor
/// zone1970.tab and whose territory lies in a different country than their
/// link target. Verified against tzdata geography (Svalbard/Jan Mayen are
/// Norwegian territory under country code SJ; Chuuk/Truk are in the
/// Federated States of Micronesia, FM, while Port Moresby is in Papua New
/// Guinea, PG).
const LINK_COUNTRIES: &[(&str, &str)] = &[("Atlantic/Jan_Mayen", "SJ"), ("Pacific/Truk", "FM")];

type Pairs = Vec<(String, String)>;

fn text_of(member: &Member) -> String {
    String::from_utf8_lossy(&member.data).into_owned()
}

fn parse_members(members: &[Member]) -> (BTreeSet<String>, BTreeMap<String, String>) {
    let mut zones = BTreeSet::new();
    let mut links = BTreeMap::new();
    for member in members {
        for line in text_of(member).lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts.first() {
                Some(&"Zone") if parts.len() >= 2 => {
                    zones.insert(parts[1].to_string());
                }
                Some(&"Link") if parts.len() >= 3 => {
                    links.insert(parts[2].to_string(), parts[1].to_string());
                }
                _ => {}
            }
        }
    }
    (zones, links)
}

fn parse_zone_tab(members: &[Member], name: &str) -> Pairs {
    let mut rows = Vec::new();
    for member in members
        .iter()
        .filter(|member| basename(&member.name) == name)
    {
        for line in text_of(member).lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 3 {
                fields = line.split_whitespace().collect();
                if fields.len() < 3 {
                    continue;
                }
            }
            let tz = fields[2].trim();
            for code in fields[0].split(',').map(str::trim) {
                if !code.is_empty() && !tz.is_empty() {
                    rows.push((code.to_string(), tz.to_string()));
                }
            }
        }
    }
    rows
}

fn parse_backzone(members: &[Member]) -> BTreeMap<String, String> {
    let mut links = BTreeMap::new();
    for member in members
        .iter()
        .filter(|member| basename(&member.name) == "backzone")
    {
        for line in text_of(member).lines() {
            let stripped = line.trim();
            let body = match stripped.strip_prefix("#PACKRATLIST zone.tab ") {
                Some(rest) if stripped.starts_with("#PACKRATLIST zone.tab Link") => rest,
                _ => stripped,
            };
            let parts: Vec<&str> = body.split_whitespace().collect();
            if parts.first() == Some(&"Link") && parts.len() >= 3 {
                links
                    .entry(parts[2].to_string())
                    .or_insert_with(|| parts[1].to_string());
            }
        }
    }
    links
}

fn rust_list(name: &str, items: &[String]) -> String {
    let lines: Vec<String> = items
        .chunks(6)
        .map(|chunk| {
            let encoded: Vec<String> = chunk.iter().map(|item| json_string(item)).collect();
            format!("    {},", encoded.join(","))
        })
        .collect();
    format!(
        "pub(super) const {name}: &[&str] = &[\n{}\n];",
        lines.join("\n")
    )
}

fn rust_pairs(name: &str, pairs: &[(String, String)]) -> String {
    let lines: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("    ({},{}),", json_string(key), json_string(value)))
        .collect();
    format!(
        "pub(super) const {name}: &[(&str, &str)] = &[\n{}\n];",
        lines.join("\n")
    )
}

fn country_of(
    name: &str,
    zone1970: &Pairs,
    zone_tab: &Pairs,
    links: &BTreeMap<String, String>,
) -> Option<String> {
    for (code, tz) in zone1970.iter().chain(zone_tab.iter()) {
        if tz == name {
            return Some(code.clone());
        }
    }
    if let Some((_, code)) = LINK_COUNTRIES.iter().find(|(link, _)| *link == name) {
        return Some(code.to_string());
    }
    let target = links.get(name)?;
    country_of(target, zone1970, zone_tab, links)
}

struct Tables {
    zones: BTreeSet<String>,
    links: BTreeMap<String, String>,
    zone_tab: Pairs,
    zone1970: Pairs,
    backzone: BTreeMap<String, String>,
}

impl Tables {
    /// The ECMA-402 primary identifier of `identifier`, mirroring the generator's
    /// self-check; `None` when the name is not a known zone or link.
    fn primary(&self, identifier: &str) -> Option<String> {
        let canonical = self
            .zones
            .iter()
            .chain(self.links.keys())
            .find(|candidate| candidate.to_lowercase() == identifier.to_lowercase())?
            .clone();
        let mut current = canonical.clone();
        let mut hops = 0;
        while let Some(next) = self.links.get(&current).filter(|_| hops < 8) {
            current = next.clone();
            hops += 1;
        }
        let zone = current;
        let mut primary = if self.links.contains_key(&canonical)
            && !self.zone_tab.iter().any(|(_, tz)| *tz == canonical)
        {
            if zone.starts_with("Etc/") {
                Some(zone.clone())
            } else {
                let cc_id = country_of(&canonical, &self.zone1970, &self.zone_tab, &self.links);
                let cc_zone = country_of(&zone, &self.zone1970, &self.zone_tab, &self.links);
                if cc_id.is_some() && cc_id == cc_zone {
                    Some(zone.clone())
                } else {
                    let rows: Vec<&String> = self
                        .zone_tab
                        .iter()
                        .filter(|(cc, _)| Some(cc) == cc_id.as_ref())
                        .map(|(_, tz)| tz)
                        .collect();
                    if rows.len() == 1 {
                        Some(rows[0].clone())
                    } else {
                        self.backzone.get(&canonical).cloned()
                    }
                }
            }
        } else {
            Some(canonical)
        };
        if matches!(primary.as_deref(), Some("Etc/UTC" | "Etc/GMT" | "GMT")) {
            primary = Some("UTC".into());
        }
        primary
    }
}

fn run(cache: &Path, output: &Path) -> Result<(), String> {
    let archive = cache.join(format!("tzdata{VERSION}.tar.gz"));
    download_once(
        &format!("https://data.iana.org/time-zones/releases/tzdata{VERSION}.tar.gz"),
        &archive,
    )?;
    let members = read_tgz(&archive)?;
    let (zones, links) = parse_members(&members);
    let tables = Tables {
        zone_tab: parse_zone_tab(&members, "zone.tab"),
        zone1970: parse_zone_tab(&members, "zone1970.tab"),
        backzone: parse_backzone(&members),
        zones,
        links,
    };
    eprintln!(
        "zones={} links={} zone.tab={} zone1970={} backzone={}",
        tables.zones.len(),
        tables.links.len(),
        tables.zone_tab.len(),
        tables.zone1970.len(),
        tables.backzone.len()
    );

    let mut zone_names: Vec<String> = tables.zones.iter().cloned().collect();
    zone_names.sort();
    let mut link_pairs: Pairs = tables
        .links
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    link_pairs.sort();
    let mut backzone_pairs: Pairs = tables
        .backzone
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    backzone_pairs.sort();
    let country_pairs: Pairs = LINK_COUNTRIES
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect();

    let mut text =
        format!("// Generated from IANA tzdata {VERSION} by tools/generators (tz-data).\n");
    text += &rust_list("ZONE_NAMES", &zone_names);
    text += "\n";
    text += &rust_pairs("LINK_TARGETS", &link_pairs);
    text += "\n";
    text += &rust_pairs("ZONE_TAB", &tables.zone_tab);
    text += "\n";
    text += &rust_pairs("ZONE1970_TAB", &tables.zone1970);
    text += "\n";
    text += &rust_pairs("BACKZONE_LINKS", &backzone_pairs);
    text += "\n";
    text += &rust_pairs("LINK_COUNTRIES", &country_pairs);
    text += "\n";

    let pinned = [
        ("Europe/Prague", "Europe/Prague"),
        ("Europe/Bratislava", "Europe/Bratislava"),
        ("Australia/Canberra", "Australia/Sydney"),
        ("Atlantic/Jan_Mayen", "Arctic/Longyearbyen"),
        ("Pacific/Truk", "Pacific/Chuuk"),
        ("Etc/UCT", "UTC"),
        ("Etc/GMT0", "UTC"),
        ("AFRICA/ABIDJAN", "Africa/Abidjan"),
        ("utc", "UTC"),
        ("US/Eastern", "America/New_York"),
    ];
    let bad: Vec<String> = pinned
        .iter()
        .filter(|(name, expected)| tables.primary(name).as_deref() != Some(*expected))
        .map(|(name, expected)| format!("{name}: {:?} != {expected}", tables.primary(name)))
        .collect();
    if !bad.is_empty() {
        return Err(format!("primary mismatches: {}", bad.join("; ")));
    }
    eprintln!("primary self-check ok");
    write_output(output, "timezone-names.rs", &text)?;
    eprintln!("wrote {}", output.join("timezone-names.rs").display());
    Ok(())
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [cache, output] = arguments.as_slice() else {
        eprintln!("usage: tz-data <cache-dir> <output-dir>");
        return ExitCode::from(2);
    };
    match run(&PathBuf::from(cache), &PathBuf::from(output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("tz-data: {message}");
            ExitCode::FAILURE
        }
    }
}
