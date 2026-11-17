// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Run-time selection of the AVX2 kernels (the `simd` feature).
//!
//! Each kernel is a safe `#[target_feature]` function next to the scalar
//! code it replaces. The [`avx2_dispatch!`] macro wraps a call to it in a
//! function that `multiversion` compiles twice, for the x86_64 baseline
//! and for AVX2, and that picks a version once per process from the CPU's
//! features. Calling a `#[target_feature]` function from a context that
//! has those features is safe (Rust 1.86 and later), and since Rust 1.87
//! the intrinsics that take no pointers are safe inside such functions,
//! so none of this needs `unsafe`.
//!
//! Without AVX2, and on other architectures, the dispatching function
//! takes its fallback branch, which is the scalar code.
//!
//! So does a build in which `-C target-cpu` or `-C target-feature`
//! already enable all of the kernels' features (`target-cpu=native` on
//! an AVX2 machine, or `x86-64-v3`). Rust still requires the
//! `#[target_feature]` attribute on the caller of a kernel or an
//! intrinsic then, and `multiversion` then calls the function's
//! baseline version, which has no such attribute, so no safe code could
//! reach the kernels. Such builds leave the kernels out: each kernel
//! module starts with the same `cfg` as the macro's.
//!
//! Vectors are loaded and stored through arrays: `_mm256_setr_epi32` of
//! the eight elements of an `&[u32; 8]` and the eight
//! `_mm256_extract_epi32` lanes of a vector compile to single unaligned
//! loads and stores, as `_mm256_loadu_si256` and `_mm256_storeu_si256`
//! would, but without their raw pointers.

#[rustversion::before(1.87)]
compile_error!("the `simd` feature needs Rust 1.87 or later (safe intrinsics in `#[target_feature]` functions)");

/// Defines a function that runs `avx2` (an expression, usually a call to a
/// `#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]` kernel) on
/// a CPU with those features, and `fallback` elsewhere:
///
/// ```ignore
/// avx2_dispatch! {
///     pub(crate) fn add(a: &[u32], out: &mut [u32]) -> bool {
///         avx2 => { kernel(a, out); true },
///         fallback => false,
///     }
/// }
/// ```
///
/// The target is AVX2 (with the SSE levels and AVX it implies), plus
/// BMI1, BMI2, LZCNT and POPCNT, which every CPU with AVX2 has, for the
/// scalar parts of the kernels. FMA is left out on purpose: Rust never
/// contracts a multiply and an add into one, and the kernels must round
/// as the scalar code does. `multiversion` needs the target as a literal,
/// in both places below.
///
/// Where the build enables the target at compile time (see the module
/// documentation), and on other architectures, the function is the
/// fallback alone.
macro_rules! avx2_dispatch {
    (
        $(#[$attr:meta])*
        $vis:vis fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty {
            avx2 => $avx2:expr,
            fallback => $fallback:expr $(,)?
        }
    ) => {
        #[cfg(all(
            target_arch = "x86_64",
            not(all(
                target_feature = "avx2",
                target_feature = "bmi1",
                target_feature = "bmi2",
                target_feature = "lzcnt",
                target_feature = "popcnt"
            ))
        ))]
        $(#[$attr])*
        #[multiversion::multiversion(targets("x86_64+avx2+bmi1+bmi2+lzcnt+popcnt"))]
        // the fallback may ignore the arguments
        #[allow(unused_variables)]
        $vis fn $name($($arg: $ty),*) -> $ret {
            multiversion::target::match_target! {
                "x86_64+avx2+bmi1+bmi2+lzcnt+popcnt" => $avx2,
                _ => $fallback,
            }
        }

        #[cfg(not(all(
            target_arch = "x86_64",
            not(all(
                target_feature = "avx2",
                target_feature = "bmi1",
                target_feature = "bmi2",
                target_feature = "lzcnt",
                target_feature = "popcnt"
            ))
        )))]
        $(#[$attr])*
        #[inline(always)]
        #[allow(unused_variables)]
        $vis fn $name($($arg: $ty),*) -> $ret {
            $fallback
        }
    };
}
#[allow(unused_imports)] // until the first kernel
pub(crate) use avx2_dispatch;

avx2_dispatch! {
    /// Whether the AVX2 kernels run on this CPU (for the tests).
    #[cfg(test)]
    pub(crate) fn active() -> bool {
        avx2 => true,
        fallback => false,
    }
}

#[cfg(test)]
mod tests {
    /// The kernels run where the CPU has the features (so the tests with
    /// the feature exercise them), and only there, unless the build
    /// enables them at compile time.
    #[test]
    fn dispatch_follows_the_cpu() {
        #[cfg(target_arch = "x86_64")]
        let expected = std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("bmi1")
            && std::arch::is_x86_feature_detected!("bmi2")
            && std::arch::is_x86_feature_detected!("lzcnt")
            && std::arch::is_x86_feature_detected!("popcnt")
            && !cfg!(all(
                target_feature = "avx2",
                target_feature = "bmi1",
                target_feature = "bmi2",
                target_feature = "lzcnt",
                target_feature = "popcnt"
            ));
        #[cfg(not(target_arch = "x86_64"))]
        let expected = false;
        assert_eq!(super::active(), expected);
    }
}
