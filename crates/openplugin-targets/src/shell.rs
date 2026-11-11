//! `export_shell!` emits the C ABI in [`openplugin_ffi::shell`].
//!
//! A native VST3, Audio Unit, or AAX adapter links the plugin static library and
//! calls `op_shell_*`. One plugin binary exports one shell.

pub use openplugin_ffi::shell::{
    create, destroy, get_normalized, get_plain, latency, process, reset, set_normalized, set_plain,
    to_normalized, to_plain, write_info, write_param, OpParamDesc, OpShellInfo, ShellInstance, ABI,
    AAX_MAX_PARAMS, PARAM_AUTOMATABLE, PARAM_DISCRETE, PARAM_LOG,
};

/// Export `op_shell_*` from the plugin crate. `$plugin` implements [`Plugin`](crate::PluginForExport)
/// and [`Default`]. Link the staticlib into `native/`.
#[macro_export]
macro_rules! export_shell {
    ($plugin:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn op_shell_info(out: *mut $crate::shell::OpShellInfo) -> u32 {
            $crate::shell::write_info::<$plugin>(out)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_param(
            index: u32,
            out: *mut $crate::shell::OpParamDesc,
        ) -> u32 {
            $crate::shell::write_param::<$plugin>(index, out)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_to_plain(index: u32, normalized: f64) -> f64 {
            $crate::shell::to_plain::<$plugin>(index, normalized)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_to_normalized(index: u32, plain: f64) -> f64 {
            $crate::shell::to_normalized::<$plugin>(index, plain)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_create(
            sample_rate: f64,
            max_block: u32,
            channels: u32,
        ) -> *mut ::std::ffi::c_void {
            $crate::shell::create::<$plugin>(sample_rate, max_block, channels)
                as *mut ::std::ffi::c_void
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_destroy(instance: *mut ::std::ffi::c_void) {
            $crate::shell::destroy(instance as *mut $crate::shell::ShellInstance);
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_reset(instance: *mut ::std::ffi::c_void) {
            $crate::shell::reset(instance as *mut $crate::shell::ShellInstance);
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_latency(instance: *const ::std::ffi::c_void) -> u32 {
            $crate::shell::latency(instance as *const $crate::shell::ShellInstance)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_set_plain(
            instance: *mut ::std::ffi::c_void,
            index: u32,
            plain: f32,
        ) -> u32 {
            $crate::shell::set_plain(instance as *mut $crate::shell::ShellInstance, index, plain)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_get_plain(
            instance: *const ::std::ffi::c_void,
            index: u32,
        ) -> f32 {
            $crate::shell::get_plain(instance as *const $crate::shell::ShellInstance, index)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_set_normalized(
            instance: *mut ::std::ffi::c_void,
            index: u32,
            normalized: f32,
        ) -> u32 {
            $crate::shell::set_normalized(
                instance as *mut $crate::shell::ShellInstance,
                index,
                normalized,
            )
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_get_normalized(
            instance: *const ::std::ffi::c_void,
            index: u32,
        ) -> f32 {
            $crate::shell::get_normalized(instance as *const $crate::shell::ShellInstance, index)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_process(
            instance: *mut ::std::ffi::c_void,
            in_left: *const f32,
            in_right: *const f32,
            out_left: *mut f32,
            out_right: *mut f32,
            frames: u32,
        ) -> u32 {
            $crate::shell::process(
                instance as *mut $crate::shell::ShellInstance,
                in_left,
                in_right,
                out_left,
                out_right,
                frames,
            )
        }
    };
}
