/*
 * Bindgen only parses FLINT's inline bodies; MSVC compiles the library itself.
 * Clang's headers lack these MSVC intrinsics, so provide their declarations
 * without an implementation. The per-header allowlist keeps them out of the
 * generated Rust bindings.
 * https://learn.microsoft.com/en-us/cpp/intrinsics/udiv128
 * https://learn.microsoft.com/en-us/cpp/intrinsics/div128
 */
#if defined(__clang__) && defined(_MSC_VER) && defined(_M_X64)
#include <immintrin.h>

unsigned __int64 _udiv128(unsigned __int64 highDividend,
                        unsigned __int64 lowDividend,
                        unsigned __int64 divisor,
                        unsigned __int64 *remainder);
__int64 _div128(__int64 highDividend, __int64 lowDividend,
               __int64 divisor, __int64 *remainder);
#endif
