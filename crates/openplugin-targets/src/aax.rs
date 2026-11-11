//! AAX descriptor types.
//!
//! A host cannot load these. The loadable shell is `native/aax`, over the `op_shell_*` ABI.
//! Building it needs an external Avid AAX SDK. The SDK is not copied into this tree.

use openplugin_core::Plugin;

#[repr(C)]
pub struct AaxComponentDescriptor {
    pub effect_id: *const u8,
    pub effect_name: *const u8,
}

pub struct AaxAdapter<P: Plugin> {
    pub inner: P,
}

impl<P: Plugin> AaxAdapter<P> {
    pub fn new(inner: P) -> Self {
        Self { inner }
    }

    /// Prepares AAX effect description and registers parameters with host.
    pub fn describe_effect(&self) -> AaxComponentDescriptor {
        let info = self.inner.info();
        AaxComponentDescriptor {
            effect_id: info.id.as_ptr(),
            effect_name: info.name.as_ptr(),
        }
    }
}
