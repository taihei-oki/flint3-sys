use flint3_sys::*;

unsafe extern "C" fn square(index: slong, values: *mut c_void) {
    // FLINT gives each index to exactly one worker and joins the work before
    // flint_parallel_do returns, so these writes do not overlap.
    values
        .cast::<slong>()
        .add(index as usize)
        .write(index * index);
}

#[test]
fn native_thread_pool_runs_and_joins_work() {
    // Keep this in its own test binary: configuring and cleaning up FLINT's
    // global pool must not race other tests. This also exercises initialization
    // of statically linked PThreads4W, thread creation, and condition variables.
    let mut values: [slong; 32] = [-1; 32];
    let num_threads;
    unsafe {
        flint_set_num_threads(2);
        num_threads = flint_get_num_threads();
        flint_parallel_do(
            Some(square),
            values.as_mut_ptr().cast(),
            values.len() as slong,
            2,
            0,
        );
        flint_cleanup_master();
    }

    assert_eq!(num_threads, 2);
    for (index, value) in values.into_iter().enumerate() {
        assert_eq!(value, (index * index) as slong);
    }
}
