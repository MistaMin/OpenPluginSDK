//! One build of the identity pinning proof's plugin (`examples/pinned`): no pins, the second parameter order (the negative control).
//! Exports the shell ABI for the native proofs.

openplugin::targets::export_shell!(example_pinned::PlainB);
