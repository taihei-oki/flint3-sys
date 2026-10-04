use std::{ffi::CStr, mem::size_of, ptr::null_mut};

use flint3_sys::*;

fn check_word_widths() {
    assert_eq!(size_of::<slong>(), size_of::<isize>());
    assert_eq!(size_of::<ulong>(), size_of::<usize>());
    assert_eq!(size_of::<fmpz>(), size_of::<isize>());
}

unsafe fn decimal_string(value: &fmpz) -> String {
    let text = fmpz_get_str(null_mut(), 10, value);
    let result = CStr::from_ptr(text).to_string_lossy().into_owned();
    flint_free(text.cast());
    result
}

#[test]
fn signed_and_unsigned_words_preserve_all_bits() {
    // Check before passing storage to C, so a wrong alias fails without
    // allowing FLINT to write past the end of a Rust fmpz.
    check_word_widths();

    unsafe {
        let mut value: fmpz = Default::default();
        fmpz_init(&mut value);

        fmpz_set_ui(&mut value, ulong::MAX);
        let unsigned = fmpz_get_ui(&value);
        let unsigned_text = decimal_string(&value);

        fmpz_set_si(&mut value, slong::MIN);
        let signed = fmpz_get_si(&value);
        let signed_text = decimal_string(&value);
        fmpz_clear(&mut value);

        assert_eq!(unsigned, ulong::MAX);
        assert_eq!(unsigned_text, ulong::MAX.to_string());
        assert_eq!(signed, slong::MIN);
        assert_eq!(signed_text, slong::MIN.to_string());
    }
}

#[test]
fn polynomial_coefficients_preserve_full_word_moduli() {
    check_word_widths();

    unsafe {
        let mut poly: nmod_poly_struct = Default::default();
        nmod_poly_init(&mut poly, ulong::MAX);
        nmod_poly_set_coeff_ui(&mut poly, 2, ulong::MAX - 1);
        let coefficient = nmod_poly_get_coeff_ui(&poly, 2);
        let length = nmod_poly_length(&poly);
        nmod_poly_clear(&mut poly);

        assert_eq!(coefficient, ulong::MAX - 1);
        assert_eq!(length, 3);
    }
}

#[test]
fn double_word_helpers_use_the_native_calling_convention() {
    check_word_widths();

    // FLINT represents ull_t as u128 with GCC and as a two-limb struct with
    // MSVC. Exercise arguments and return values without assuming either
    // representation in Rust.
    unsafe {
        assert_eq!(ull_hi(ull(7, ulong::MAX)), 7);
        assert_eq!(ull_lo(ull(7, ulong::MAX)), ulong::MAX);
        assert_eq!(ull_hi(ull_add_u(ull(7, ulong::MAX), 1)), 8);
        assert_eq!(ull_lo(ull_add_u(ull(7, ulong::MAX), 1)), 0);
        assert_eq!(ull_hi(ull_u_mul_u(ulong::MAX, 2)), 1);
        assert_eq!(ull_lo(ull_u_mul_u(ulong::MAX, 2)), ulong::MAX - 1);
    }
}

#[test]
fn quadratic_sieve_fields_follow_the_native_mutex_layout() {
    check_word_widths();

    unsafe {
        let mut n: fmpz = Default::default();
        let mut sieve: qs_s = Default::default();
        fmpz_init(&mut n);
        fmpz_set_ui(&mut n, 65537);
        qsieve_init_with_tune(&mut sieve, &n, 17, 101, 7, 65536, 53);

        // These fields occur after pthread_mutex_t in qs_s. Reading values
        // written by C checks offsets for libc, winpthreads, and PThreads4W.
        // This small fmpz is inline, so inspecting n[0] needs no dereference
        // through a potentially misplaced GMP pointer.
        let fields = (
            sieve.n[0],
            sieve.bits,
            sieve.ks_primes,
            sieve.fb_primes,
            sieve.small_primes,
            sieve.sieve_size,
            sieve.sieve_bits,
            sieve.sieve_fill,
        );
        let has_filename_buffer = !sieve.fname.is_null();
        qsieve_clear(&mut sieve);
        fmpz_clear(&mut n);

        assert_eq!(fields, (65537, 17, 17, 101, 7, 65536, 64, 11));
        assert!(has_filename_buffer);
    }
}
