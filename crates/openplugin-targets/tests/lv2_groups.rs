//! Parameter groups as LV2 port groups: one `pg:Group` per group path,
//! nested with `pg:subGroupOf`, and `pg:group` on each control port of a
//! grouped parameter. A plugin without groups is described as before.

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::lv2;

static PARAMS: [ParamInfo; 5] = [
    ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None),
    ParamInfo::new("cutoff", "Cutoff", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_group("Filter"),
    ParamInfo::new("attack", "Attack", 0.0, 1.0, 0.01, Unit::Seconds).with_group("Filter/Envelope"),
    ParamInfo::new("drive", "Drive", 0.0, 1.0, 0.0, Unit::None).with_group("Filter_Envelope"),
    ParamInfo::new("broken", "Broken", 0.0, 1.0, 0.0, Unit::None).with_group("Filter//Envelope"),
];

static INFO: PluginInfo = PluginInfo {
    id: "urn:openplugin:test-grouped",
    name: "Test Grouped",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

static PLAIN_PARAMS: [ParamInfo; 1] = [ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None)];
static PLAIN_INFO: PluginInfo = PluginInfo { id: "urn:openplugin:test-plain", name: "Test Plain", params: &PLAIN_PARAMS, ..INFO };

struct Grouped(&'static PluginInfo);

impl Plugin for Grouped {
    fn info(&self) -> &'static PluginInfo {
        self.0
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, _: f32) -> bool {
        self.0.param(id).is_some()
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.0.param(id).map(|p| p.default)
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
}

/// The control port block of the parameter with `symbol`.
fn port_block<'a>(ttl: &'a str, symbol: &str) -> &'a str {
    let at = ttl.find(&format!("lv2:symbol \"{symbol}\"")).unwrap_or_else(|| panic!("no port {symbol}"));
    let start = ttl[..at].rfind('[').unwrap();
    let end = at + ttl[at..].find(']').unwrap();
    &ttl[start..end]
}

/// The description of the group resource `<uri#group_{symbol}>`.
fn group_block<'a>(ttl: &'a str, symbol: &str) -> &'a str {
    let head = format!("\n<urn:openplugin:test-grouped#group_{symbol}>\n");
    let at = ttl.find(&head).unwrap_or_else(|| panic!("no group {symbol} in\n{ttl}"));
    let end = at + ttl[at..].find(" .\n").unwrap();
    &ttl[at..end]
}

#[test]
fn groups_are_port_groups_and_nested_groups_name_their_parent() {
    let ttl = lv2::plugin_ttl(&Grouped(&INFO), false);
    assert!(ttl.contains("@prefix pg: <http://lv2plug.in/ns/ext/port-groups#> ."));
    // Three groups: Filter, Filter/Envelope, and Filter_Envelope (whose symbol
    // would clash with the nested one's, so it gets a number).
    assert_eq!(ttl.matches("a pg:Group , pg:InputGroup").count(), 3, "{ttl}");
    let filter = group_block(&ttl, "Filter");
    assert!(filter.contains("lv2:symbol \"Filter\" ;"));
    assert!(filter.contains("lv2:name \"Filter\""));
    assert!(!filter.contains("pg:subGroupOf"), "a top-level group has no parent");
    let envelope = group_block(&ttl, "Filter_Envelope");
    assert!(envelope.contains("lv2:name \"Envelope\""));
    assert!(envelope.contains("pg:subGroupOf <urn:openplugin:test-grouped#group_Filter>"));
    let other = group_block(&ttl, "Filter_Envelope_2");
    assert!(other.contains("lv2:name \"Filter_Envelope\""));
    assert!(!other.contains("pg:subGroupOf"));

    assert!(!port_block(&ttl, "level").contains("pg:group"), "a top-level parameter has no group");
    assert!(port_block(&ttl, "cutoff").contains("pg:group <urn:openplugin:test-grouped#group_Filter>"));
    assert!(port_block(&ttl, "attack").contains("pg:group <urn:openplugin:test-grouped#group_Filter_Envelope>"));
    assert!(port_block(&ttl, "drive").contains("pg:group <urn:openplugin:test-grouped#group_Filter_Envelope_2>"));
    assert!(!port_block(&ttl, "broken").contains("pg:group"), "an invalid path is the top level");
    assert_eq!(ttl.matches("pg:group <").count(), 3);
}

#[test]
fn a_plugin_without_groups_has_no_port_groups() {
    let ttl = lv2::plugin_ttl(&Grouped(&PLAIN_INFO), false);
    assert!(!ttl.contains("pg:"), "{ttl}");
    assert!(!ttl.contains("port-groups"), "{ttl}");
}

/// The grouped plugin's bundle passes `lv2_validate` when it is installed (CI installs it).
#[test]
fn grouped_ttl_passes_lv2_validate_when_installed() {
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("lv2-groups-bundle");
    let dir = root.join("grouped.lv2");
    std::fs::create_dir_all(&dir).unwrap();
    lv2::write_bundle(&Grouped(&INFO), &dir, "grouped", false).unwrap();
    let Ok(output) = std::process::Command::new("lv2_validate").current_dir(&dir).args(["manifest.ttl", "grouped.ttl"]).output() else {
        eprintln!("lv2_validate is not installed; skipped");
        return;
    };
    let log = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(log.contains("Found 0 errors"), "{log}");
    assert!(!log.lines().any(|l| l.starts_with("error")), "{log}");
    assert!(!log.lines().any(|l| l.starts_with("Skipping file") && l.contains("grouped")), "{log}");
}

/// lilv reads the groups back (through `lv2info`, when it is installed): the
/// group of each port as a host sees it.
#[test]
fn lilv_reads_each_ports_group_when_installed() {
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("lv2-groups-lilv");
    let dir = root.join("grouped.lv2");
    std::fs::create_dir_all(&dir).unwrap();
    lv2::write_bundle(&Grouped(&INFO), &dir, "grouped", false).unwrap();
    let Ok(output) = std::process::Command::new("lv2info").env("LV2_PATH", &root).arg(INFO.id).output() else {
        eprintln!("lv2info is not installed; skipped");
        return;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    // Symbol, then the group lilv found for that port ("" for none).
    let mut seen: Vec<(String, String)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(symbol) = line.strip_prefix("Symbol:") {
            seen.push((symbol.trim().to_string(), String::new()));
        } else if let (Some(group), Some(last)) = (line.strip_prefix("Group:"), seen.last_mut()) {
            last.1 = group.trim().to_string();
        }
    }
    let group_of = |symbol: &str| {
        seen.iter().find(|(s, _)| s == symbol).map(|(_, g)| g.clone()).unwrap_or_else(|| panic!("no port {symbol} in\n{text}"))
    };
    assert_eq!(group_of("level"), "");
    assert_eq!(group_of("cutoff"), "urn:openplugin:test-grouped#group_Filter");
    assert_eq!(group_of("attack"), "urn:openplugin:test-grouped#group_Filter_Envelope");
    assert_eq!(group_of("drive"), "urn:openplugin:test-grouped#group_Filter_Envelope_2");
    assert_eq!(group_of("broken"), "");
    assert_eq!(group_of("freewheel"), "");
}
