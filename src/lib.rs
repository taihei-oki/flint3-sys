#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unnecessary_transmutes)]

pub use libc::{
    c_char, c_double, c_int, c_long, c_longlong, c_schar, c_short, c_uchar, c_uint, c_ulong,
    c_ulonglong, c_ushort, c_void,
};

pub use libc::FILE;

#[cfg(not(windows))]
pub use libc::pthread_mutex_t;

// MSYS2's winpthreads defines pthread_mutex_t as intptr_t.
#[cfg(all(windows, not(target_env = "msvc")))]
pub type pthread_mutex_t = isize;

// vcpkg's PThreads4W defines pthread_mutex_t as a pointer to an opaque struct.
#[cfg(all(windows, target_env = "msvc"))]
#[repr(C)]
pub struct pthread_mutex_t_ {
    _unused: [u8; 0],
}
#[cfg(all(windows, target_env = "msvc"))]
pub type pthread_mutex_t = *mut pthread_mutex_t_;

// FLINT limbs follow the pointer width, including Win64's LLP64 ABI where
// C long is only 32 bits. Keep these aliases outside generated bindings so
// bindings generated on Unix can also be used on Windows.
#[cfg(not(all(windows, target_pointer_width = "64")))]
pub use libc::{c_long as slong, c_ulong as ulong};
#[cfg(all(windows, target_pointer_width = "64"))]
pub use libc::{c_longlong as slong, c_ulonglong as ulong};

pub type size_t = libc::size_t;
pub type ssize_t = libc::ssize_t;

pub type __va_list_tag = u64;

include!(concat!(env!("OUT_DIR"), "/flint.rs"));
