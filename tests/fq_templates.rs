use flint3_sys::*;

#[test]
fn fq_template_matrix_and_polynomial_families_are_bound() {
    // These public APIs are instantiated through FLINT's matrix and polynomial
    // template headers. Merely naming a representative function from every
    // family catches the bindgen file-allowlist failure this test guards.
    let _ = fq_mat_init;
    let _ = fq_mat_rank;
    let _ = fq_poly_init;
    let _ = fq_poly_length;
    let _ = fq_nmod_mat_init;
    let _ = fq_nmod_mat_rank;
    let _ = fq_nmod_poly_init;
    let _ = fq_nmod_poly_length;
    let _ = fq_zech_mat_init;
    let _ = fq_zech_mat_rank;
    let _ = fq_zech_poly_init;
    let _ = fq_zech_poly_length;

    // Exercise one family against FLINT as well as checking that it was
    // generated. The other two families share the same template mechanism.
    unsafe {
        let mut generator: nmod_poly_struct = Default::default();
        let mut context: fq_nmod_ctx_struct = Default::default();
        let mut matrix: fq_nmod_mat_struct = Default::default();
        let mut polynomial: fq_nmod_poly_struct = Default::default();

        nmod_poly_init(&mut generator, 3);
        nmod_poly_set_coeff_ui(&mut generator, 0, 1);
        nmod_poly_set_coeff_ui(&mut generator, 2, 1);
        fq_nmod_ctx_init_modulus(&mut context, &generator, c"a".as_ptr());

        fq_nmod_mat_init(&mut matrix, 2, 2, &context);
        assert_eq!(fq_nmod_mat_rank(&matrix, &context), 0);

        fq_nmod_poly_init(&mut polynomial, &context);
        assert_eq!(fq_nmod_poly_length(&polynomial, &context), 0);

        fq_nmod_poly_clear(&mut polynomial, &context);
        fq_nmod_mat_clear(&mut matrix, &context);
        fq_nmod_ctx_clear(&mut context);
        nmod_poly_clear(&mut generator);
    }
}
