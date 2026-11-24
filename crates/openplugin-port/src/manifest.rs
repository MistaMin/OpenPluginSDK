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

/// What a scan found out about one plugin: the JSON document `openplugin-port scan` writes.
/// Serialise and parse it with `serde_json`; `docs/PORTING.md` describes every field.
///
/// ```
/// use openplugin_port::manifest::{Manifest, Plugin, Source, SCHEMA_VERSION};
///
/// let manifest = Manifest {
///     schema_version: SCHEMA_VERSION,
///     tool: openplugin_port::tool_id(),
///     source: Source { format: "clap".into(), path: "Verb.clap".into(), count: 1, ..Default::default() },
///     plugin: Plugin { name: "Verb".into(), vendor: "Acme".into(), ..Default::default() },
/// };
/// let json = serde_json::to_string_pretty(&manifest).unwrap();
/// let back: Manifest = serde_json::from_str(&json).unwrap();
/// assert_eq!(back, manifest);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`SCHEMA_VERSION`] of the writer. A reader refuses a higher number than it knows.
    pub schema_version: u32,
    /// `openplugin-port <version>`.
    pub tool: String,
    /// Where the plugin was loaded from.
    pub source: Source,
    /// What the plugin reported about itself.
    pub plugin: Plugin,
}

/// Where a scanned plugin came from: the format, the path, and its place in the bundle.
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

/// One plugin as its format reports it: identity, ids, buses, latency, tail and parameters.
/// Fields the plugin did not report are empty, `null` (`None`) or zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Plugin {
    /// Display name.
    pub name: String,
    /// Vendor or manufacturer name.
    pub vendor: String,
    /// Version text, as the plugin gives it.
    pub version: String,
    /// Vendor or product URL; empty when none.
    pub url: String,
    /// Support e-mail address; empty when none.
    pub email: String,
    /// One-line description; empty when none.
    pub description: String,
    /// CLAP features, VST3 sub-categories (split at `|`), or the AU type name.
    pub categories: Vec<String>,
    /// True for an instrument (CLAP `instrument` feature, VST3 `Instrument`, AU `aumu`).
    pub is_instrument: bool,
    /// The plugin's ids in its own format.
    pub ids: Ids,
    /// Audio buses, inputs and outputs, in the format's order.
    pub buses: Vec<Bus>,
    /// Audio Unit `SupportedNumChannels`, when the unit reports it.
    pub channel_layouts: Vec<ChannelLayout>,
    /// Reported latency in samples; `null` when the plugin could not say.
    pub latency_samples: Option<u32>,
    /// Reported tail in samples; 4294967295 means endless; `null` when not reported.
    pub tail_samples: Option<u32>,
    /// Audio Unit tail in seconds (the Audio Unit API reports seconds).
    pub tail_seconds: Option<f64>,
    /// Parameters, in the format's order.
    pub params: Vec<Param>,
    /// Things the scan could not read or had to guess. Empty when all went well.
    pub warnings: Vec<String>,
}

/// The plugin's ids. Only the scanned format's entry is set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ids {
    /// CLAP id, for a CLAP scan.
    pub clap: Option<ClapIds>,
    /// VST3 class ids and flags, for a VST3 scan.
    pub vst3: Option<Vst3Ids>,
    /// Audio Unit component description, for an AU scan.
    pub au: Option<AuIds>,
    /// AAX ids, for an AAX bundle.
    pub aax: Option<AaxIds>,
}

/// CLAP identity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClapIds {
    /// The CLAP plugin id, such as `com.acme.verb`.
    pub id: String,
}

/// VST3 identity: class ids and the `PClassInfo2` fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Vst3Ids {
    /// The audio-module class id: 32 upper-case hex digits, the 16 bytes of the TUID in order.
    pub class_id: String,
    /// The controller class id from `IComponent::getControllerClassId`; `null` for a combined component.
    pub controller_class_id: Option<String>,
    /// `PClassInfo2::category`, normally `Audio Module Class`.
    pub category: String,
    /// `PClassInfo2::subCategories`, `|`-separated, such as `Fx|Reverb`.
    pub sub_categories: String,
    /// `PClassInfo2::classFlags`.
    pub class_flags: u32,
    /// `PClassInfo2::sdkVersion`, such as `VST 3.7.9`.
    pub sdk_version: String,
    /// True when the processor and the controller are one object.
    pub single_component: bool,
    /// True when the bundle carries `Contents/Resources/moduleinfo.json`.
    pub module_info: bool,
    /// The class entry of `moduleinfo.json` with the same CID, when both exist, as found.
    pub module_info_class: Option<serde_json::Value>,
}

/// Audio Unit identity: the `AudioComponentDescription` codes as four-character text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuIds {
    /// Component type, four characters: `aufx`, `aumu`, `aumf`...
    pub component_type: String,
    /// Component subtype, four characters.
    pub subtype: String,
    /// Manufacturer code, four characters.
    pub manufacturer: String,
    /// The `factoryFunction` named in the bundle's `Info.plist`, if any.
    pub factory_function: Option<String>,
    /// The `AudioComponents` version number.
    pub version: Option<u32>,
}

/// AAX identity, read from the bundle without loading it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AaxIds {
    /// Manufacturer id (`manID`).
    pub manufacturer: String,
    /// Product id (`prodID`).
    pub product: String,
    /// Plug-in type ids, in the order the page table lists them.
    pub plugin_ids: Vec<String>,
    /// Where the ids were read: `page-table`, or `info-plist`.
    pub found_in: String,
}

/// One audio bus.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bus {
    /// `input` or `output`.
    pub direction: String,
    /// `main` or `aux`.
    pub kind: String,
    /// Bus name; empty when none.
    pub name: String,
    /// Channel count.
    pub channels: u32,
    /// VST3 speaker arrangement as hex, or the CLAP port type.
    pub arrangement: Option<String>,
    /// `mono`, `stereo`, or `null`.
    pub layout: Option<String>,
}

/// One Audio Unit `AUChannelInfo` entry: an input and output channel count the unit accepts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelLayout {
    /// Input channels; -1 means any, -2 means any but equal to the output.
    pub inputs: i32,
    /// Output channels; -1 means any.
    pub outputs: i32,
}

/// One parameter as the format reports it.
///
/// ```
/// use openplugin_port::manifest::Param;
///
/// let param = Param { name: "Mix".into(), flags: vec!["automatable".into()], ..Default::default() };
/// assert!(param.has_flag("automatable"));
/// assert!(!param.has_flag("hidden"));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Param {
    /// The format's own parameter id (CLAP `param_id`, VST3 `ParamID`, AU `AudioUnitParameterID`).
    pub id: u32,
    /// Display name.
    pub name: String,
    /// Short name, when the format has one (VST3 `shortTitle`).
    pub short_name: Option<String>,
    /// CLAP module path, VST3 unit id as text, or AU clump id.
    pub group: Option<String>,
    /// Unit text from the format (VST3 `units`, AU unit name); empty when none.
    pub units: String,
    /// Plain values as the format reports them. For VST3 these come from
    /// `normalizedParamToPlain` at 0 and 1; a plugin that does not implement
    /// it reports the normalised range.
    pub min: f64,
    /// Plain maximum (see [`Param::min`]).
    pub max: f64,
    /// Plain default value.
    pub default: f64,
    /// Default as normalised 0..1 travel.
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

/// A parameter's value text at one normalised point.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// The normalised point, one of [`SAMPLE_POINTS`].
    pub normalized: f64,
    /// The plain value at that point (the format's own mapping).
    pub plain: f64,
    /// The value string; empty when the plugin gave none.
    pub text: String,
}

impl Param {
    /// True when [`Param::flags`] holds `name` (exact match).
    pub fn has_flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }
}

/// Hex of a 16-byte class id, upper case: 32 characters, no separators.
///
/// ```
/// assert_eq!(openplugin_port::manifest::hex16(&[0xAB; 16]), "AB".repeat(16));
/// ```
pub fn hex16(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// The result of one child probe: every plugin the bundle holds, and any note.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeResult {
    /// Every plugin the bundle holds, in its order.
    pub plugins: Vec<Plugin>,
    /// Problems the child met that are not about one plugin.
    pub notes: Vec<String>,
}
