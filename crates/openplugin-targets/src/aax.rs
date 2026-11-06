//! Avid Audio eXtension (AAX) target bridge for Pro Tools parity.

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
