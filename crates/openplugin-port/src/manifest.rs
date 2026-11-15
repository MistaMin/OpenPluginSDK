//! The manifest: what a scan found out about one plugin. See `docs/PORTING.md`
//! for the schema, field by field. The shape is versioned by
//! [`SCHEMA_VERSION`]; a reader must refuse a higher major number than it knows.

use serde::{Deserialize, Serialize};

/// Version of the manifest layout. Bumped when a field changes meaning or goes away.
pub const SCHEMA_VERSION: u32 = 1;

/// Normalised positions at which every parameter's value string is read.
pub const SAMPLE_POINTS: [f64; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// Discrete parameters with at most this many positions have every position's text read.
pub const MAX_STEP_TEXTS: u32 = 64;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    /// `openplugin-port <version>`.
    pub tool: String,
    pub source: Source,
    pub plugin: Plugin,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// `clap`, `vst3`, `au` or `aax`.
    pub format: String,
    /// The path the scan loaded, as given.
    pub path: String,
    /// Index of this plugin among those the bundle holds (a bundle may hold several).
    pub index: u32,
    /// How many plugins the bundle holds.
    pub count: u32,
    /// `CFBundleIdentifier` of the bundle, when it has an `Info.plist`.
    pub bundle_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Plugin {
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub url: String,
    pub email: String,
    pub description: String,
    /// CLAP features, VST3 sub-categories (split at `|`), or the AU type name.
    pub categories: Vec<String>,
    pub is_instrument: bool,
    pub ids: Ids,
    pub buses: Vec<Bus>,
    /// Audio Unit `SupportedNumChannels`, when the unit reports it.
    pub channel_layouts: Vec<ChannelLayout>,
    /// Reported latency in samples; `null` when the plugin could not say.
    pub latency_samples: Option<u32>,
    /// Reported tail in samples; 4294967295 means endless; `null` when not reported.
    pub tail_samples: Option<u32>,
    /// Audio Unit tail in seconds (the Audio Unit API reports seconds).
    pub tail_seconds: Option<f64>,
    pub params: Vec<Param>,
    /// Things the scan could not read or had to guess. Empty when all went well.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ids {
    pub clap: Option<ClapIds>,
    pub vst3: Option<Vst3Ids>,
    pub au: Option<AuIds>,
    pub aax: Option<AaxIds>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClapIds {
    /// The CLAP plugin id, such as `com.acme.verb`.
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Vst3Ids {
    /// The audio-module class id: 32 upper-case hex digits, the 16 bytes of the TUID in order.
    pub class_id: String,
    /// The controller class id from `IComponent::getControllerClassId`; `null` for a combined component.
    pub controller_class_id: Option<String>,
    pub category: String,
    pub sub_categories: String,
    pub class_flags: u32,
    pub sdk_version: String,
    /// True when the processor and the controller are one object.
    pub single_component: bool,
    /// True when the bundle carries `Contents/Resources/moduleinfo.json`.
    pub module_info: bool,
    /// The class entry of `moduleinfo.json` with the same CID, when both exist, as found.
    pub module_info_class: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuIds {
    /// Component type, four characters: `aufx`, `aumu`, `aumf`...
    pub component_type: String,
    pub subtype: String,
    pub manufacturer: String,
    /// The `factoryFunction` named in the bundle's `Info.plist`, if any.
    pub factory_function: Option<String>,
    /// The `AudioComponents` version number.
    pub version: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AaxIds {
    pub manufacturer: String,
    pub product: String,
    /// Plug-in type ids, in the order the page table lists them.
    pub plugin_ids: Vec<String>,
    /// Where the ids were read: `page-table`, or `info-plist`.
    pub found_in: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bus {
    /// `input` or `output`.
    pub direction: String,
    /// `main` or `aux`.
    pub kind: String,
    pub name: String,
    pub channels: u32,
    /// VST3 speaker arrangement as hex, or the CLAP port type.
    pub arrangement: Option<String>,
    /// `mono`, `stereo`, or `null`.
    pub layout: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelLayout {
    /// Input channels; -1 means any, -2 means any but equal to the output.
    pub inputs: i32,
    pub outputs: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Param {
    /// The format's own parameter id (CLAP `param_id`, VST3 `ParamID`, AU `AudioUnitParameterID`).
    pub id: u32,
    pub name: String,
    pub short_name: Option<String>,
    /// CLAP module path, VST3 unit id as text, or AU clump id.
    pub group: Option<String>,
    /// Unit text from the format (VST3 `units`, AU unit name); empty when none.
    pub units: String,
    /// Plain values as the format reports them. For VST3 these come from
    /// `normalizedParamToPlain` at 0 and 1; a plugin that does not implement
    /// it reports the normalised range.
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub default_normalized: f64,
    /// 0 for continuous, 1 for a toggle, N for N+1 positions (the VST3 meaning).
    pub step_count: u32,
    /// Flag names: `automatable`, `stepped`, `hidden`, `read_only`, `bypass`,
    /// `list`, `wrap_around`, `program_change`, `requires_process`, `non_realtime`,
    /// `meter`, `log_display`, `exp_display`, `periodic`.
    pub flags: Vec<String>,
    /// The format's flag word, unchanged.
    pub flags_raw: u32,
    /// Value strings at [`SAMPLE_POINTS`].
    pub samples: Vec<Sample>,
    /// For discrete parameters with few positions, the text at each position in order.
    pub step_texts: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub normalized: f64,
    /// The plain value at that point (the format's own mapping).
    pub plain: f64,
    /// The value string; empty when the plugin gave none.
    pub text: String,
}

impl Param {
    pub fn has_flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }
}

/// Hex of a 16-byte class id, upper case.
pub fn hex16(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// The result of one child probe: every plugin the bundle holds, and any note.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeResult {
    pub plugins: Vec<Plugin>,
    pub notes: Vec<String>,
}
