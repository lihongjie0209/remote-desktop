use core_foundation::{
    array::CFArrayRef,
    base::{CFRelease, ToVoid},
};
use std::ops::Deref;

#[derive(Debug)]
pub(super) struct BoxCFArrayRef {
    cf_array_ref: CFArrayRef,
}

impl Deref for BoxCFArrayRef {
    type Target = CFArrayRef;
    fn deref(&self) -> &Self::Target {
        &self.cf_array_ref
    }
}

impl Drop for BoxCFArrayRef {
    fn drop(&mut self) {
        unsafe {
            // Fully-qualified path required by Rust >=1.82 stricter type inference (E0282).
            CFRelease(<CFArrayRef as ToVoid<_>>::to_void(&self.cf_array_ref));
        }
    }
}

impl BoxCFArrayRef {
    pub fn new(cf_array_ref: CFArrayRef) -> Self {
        BoxCFArrayRef { cf_array_ref }
    }
}
