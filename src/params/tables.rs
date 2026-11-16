// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Static gain and visual-weight tables from `ojph_params.cpp`
//! (`sqrt_energy_gains`, `bibo_gains`, `visual_weights`).

/// Number of entries in each gain table; `num_decomp` indexes it.
pub(crate) const GAIN_TABLE_LEN: usize = 34;

pub(crate) mod sqrt_energy_gains {
    pub(crate) fn get_gain_l(num_decomp: u32, reversible: bool) -> f32 {
        let i = (num_decomp as usize).min(super::GAIN_TABLE_LEN - 1);
        if reversible {
            GAIN_5X3_L[i]
        } else {
            GAIN_9X7_L[i]
        }
    }
    pub(crate) fn get_gain_h(num_decomp: u32, reversible: bool) -> f32 {
        let i = (num_decomp as usize).min(super::GAIN_TABLE_LEN - 1);
        if reversible {
            GAIN_5X3_H[i]
        } else {
            GAIN_9X7_H[i]
        }
    }

    const GAIN_9X7_L: [f32; 34] = [
        1.0000e+00, 1.4021e+00, 2.0304e+00, 2.9012e+00, 4.1153e+00, 5.8245e+00, 8.2388e+00,
        1.1652e+01, 1.6479e+01, 2.3304e+01, 3.2957e+01, 4.6609e+01, 6.5915e+01, 9.3217e+01,
        1.3183e+02, 1.8643e+02, 2.6366e+02, 3.7287e+02, 5.2732e+02, 7.4574e+02, 1.0546e+03,
        1.4915e+03, 2.1093e+03, 2.9830e+03, 4.2185e+03, 5.9659e+03, 8.4371e+03, 1.1932e+04,
        1.6874e+04, 2.3864e+04, 3.3748e+04, 4.7727e+04, 6.7496e+04, 9.5454e+04,
    ];
    const GAIN_9X7_H: [f32; 34] = [
        1.4425e+00, 1.9669e+00, 2.8839e+00, 4.1475e+00, 5.8946e+00, 8.3472e+00, 1.1809e+01,
        1.6701e+01, 2.3620e+01, 3.3403e+01, 4.7240e+01, 6.6807e+01, 9.4479e+01, 1.3361e+02,
        1.8896e+02, 2.6723e+02, 3.7792e+02, 5.3446e+02, 7.5583e+02, 1.0689e+03, 1.5117e+03,
        2.1378e+03, 3.0233e+03, 4.2756e+03, 6.0467e+03, 8.5513e+03, 1.2093e+04, 1.7103e+04,
        2.4187e+04, 3.4205e+04, 4.8373e+04, 6.8410e+04, 9.6747e+04, 1.3682e+05,
    ];
    const GAIN_5X3_L: [f32; 34] = [
        1.0000e+00, 1.2247e+00, 1.3229e+00, 1.5411e+00, 1.7139e+00, 1.9605e+00, 2.2044e+00,
        2.5047e+00, 2.8277e+00, 3.2049e+00, 3.6238e+00, 4.1033e+00, 4.6423e+00, 5.2548e+00,
        5.9462e+00, 6.7299e+00, 7.6159e+00, 8.6193e+00, 9.7544e+00, 1.1039e+01, 1.2493e+01,
        1.4139e+01, 1.6001e+01, 1.8108e+01, 2.0493e+01, 2.3192e+01, 2.6246e+01, 2.9702e+01,
        3.3614e+01, 3.8041e+01, 4.3051e+01, 4.8721e+01, 5.5138e+01, 6.2399e+01,
    ];
    const GAIN_5X3_H: [f32; 34] = [
        1.0458e+00, 1.3975e+00, 1.4389e+00, 1.7287e+00, 1.8880e+00, 2.1841e+00, 2.4392e+00,
        2.7830e+00, 3.1341e+00, 3.5576e+00, 4.0188e+00, 4.5532e+00, 5.1494e+00, 5.8301e+00,
        6.5963e+00, 7.4663e+00, 8.4489e+00, 9.5623e+00, 1.0821e+01, 1.2247e+01, 1.3860e+01,
        1.5685e+01, 1.7751e+01, 2.0089e+01, 2.2735e+01, 2.5729e+01, 2.9117e+01, 3.2952e+01,
        3.7292e+01, 4.2203e+01, 4.7761e+01, 5.4051e+01, 6.1170e+01, 6.9226e+01,
    ];
}

pub(crate) mod bibo_gains {
    pub(crate) fn get_bibo_gain_l(num_decomp: u32, reversible: bool) -> f32 {
        let i = (num_decomp as usize).min(super::GAIN_TABLE_LEN - 1);
        if reversible {
            GAIN_5X3_L[i]
        } else {
            GAIN_9X7_L[i]
        }
    }
    pub(crate) fn get_bibo_gain_h(num_decomp: u32, reversible: bool) -> f32 {
        let i = (num_decomp as usize).min(super::GAIN_TABLE_LEN - 1);
        if reversible {
            GAIN_5X3_H[i]
        } else {
            GAIN_9X7_H[i]
        }
    }

    const GAIN_9X7_L: [f32; 34] = [
        1.0000e+00, 1.3803e+00, 1.3328e+00, 1.3067e+00, 1.3028e+00, 1.3001e+00, 1.2993e+00,
        1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00,
        1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00,
        1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00,
        1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00, 1.2992e+00,
    ];
    const GAIN_9X7_H: [f32; 34] = [
        1.2976e+00, 1.3126e+00, 1.2757e+00, 1.2352e+00, 1.2312e+00, 1.2285e+00, 1.2280e+00,
        1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00,
        1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00,
        1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00,
        1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00, 1.2278e+00,
    ];
    const GAIN_5X3_L: [f32; 34] = [
        1.0000e+00, 1.5000e+00, 1.6250e+00, 1.6875e+00, 1.6963e+00, 1.7067e+00, 1.7116e+00,
        1.7129e+00, 1.7141e+00, 1.7145e+00, 1.7151e+00, 1.7152e+00, 1.7155e+00, 1.7155e+00,
        1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00,
        1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00,
        1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00, 1.7156e+00,
    ];
    const GAIN_5X3_H: [f32; 34] = [
        2.0000e+00, 2.5000e+00, 2.7500e+00, 2.8047e+00, 2.8198e+00, 2.8410e+00, 2.8558e+00,
        2.8601e+00, 2.8628e+00, 2.8656e+00, 2.8662e+00, 2.8667e+00, 2.8669e+00, 2.8670e+00,
        2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00,
        2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00,
        2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00, 2.8671e+00,
    ];
}

/// `visual_weights` from `ojph_params.cpp`. The 1.4142 entries are
/// OpenJPH's rounded values, kept as they are.
#[allow(clippy::approx_constant)]
pub(crate) mod visual_weights {
    use super::super::qcd::CompType;
    use crate::types::Point;

    pub(crate) const VW_COLOUR_FORMAT_400: u32 = 0;
    pub(crate) const VW_COLOUR_FORMAT_420: u32 = 1;
    pub(crate) const VW_COLOUR_FORMAT_422: u32 = 2;
    pub(crate) const VW_COLOUR_FORMAT_444: u32 = 3;
    pub(crate) const VW_COLOUR_FORMAT_ERROR: u32 = 4;

    pub(crate) fn get_format(component_subsampling: &Point) -> u32 {
        let s = component_subsampling;
        if s.x == 2 && s.y == 2 {
            VW_COLOUR_FORMAT_420
        } else if s.x == 2 && s.y == 1 {
            VW_COLOUR_FORMAT_422
        } else if s.x == 1 && s.y == 1 {
            VW_COLOUR_FORMAT_444
        } else {
            VW_COLOUR_FORMAT_ERROR
        }
    }

    /// C++ asserts on an unknown format/type and returns `y`; so does this.
    pub(crate) fn get_weights(format: u32, comp_type: CompType) -> &'static [f32; 19] {
        match comp_type {
            CompType::Y => &Y,
            CompType::Cb => match format {
                VW_COLOUR_FORMAT_420 => &CB420,
                VW_COLOUR_FORMAT_422 => &CB422,
                VW_COLOUR_FORMAT_444 => &CB444,
                _ => &Y,
            },
            CompType::Cr => match format {
                VW_COLOUR_FORMAT_420 => &CR420,
                VW_COLOUR_FORMAT_422 => &CR422,
                VW_COLOUR_FORMAT_444 => &CR444,
                _ => &Y,
            },
            CompType::Undefined => &Y,
        }
    }

    pub(crate) fn get_no_weights() -> &'static [f32; 19] {
        &NO_WEIGHTS
    }

    pub(crate) fn get_weight(v: &[f32], decomposition_level: u32, subband_idx: u32) -> f32 {
        if subband_idx == 0 {
            v[18]
        } else {
            // C++ asserts 1 <= subband_idx <= 3 and decomposition_level > 0
            let d = decomposition_level.clamp(1, 6);
            let s = subband_idx.clamp(1, 3);
            let index = (d - 1) * 3 + (3 - s);
            v[index as usize]
        }
    }

    pub(crate) fn get_gain(comp_type: CompType) -> f32 {
        match comp_type {
            CompType::Y => 1.0,
            CompType::Cb => 1.8051f32 / 1.7321f32,
            CompType::Cr => 1.5734f32 / 1.7321f32,
            CompType::Undefined => 0.0,
        }
    }

    /// Returns `(delta_ref, power)`; C++ returns delta_ref and writes power
    /// through a reference.
    pub(crate) fn get_delta_ref(qfactor: f32, bit_depth: u32) -> (f32, f32) {
        const T0: f32 = 65.0;
        const T1: f32 = 97.0;
        const ALPHA_T0: f32 = 0.04;
        const ALPHA_T1: f32 = 0.10;
        const M_T0: f32 = 2.0 * (1.0 - T0 / 100.0);
        const M_T1: f32 = 2.0 * (1.0 - T1 / 100.0);

        let m_q = if qfactor < 50.0 {
            50.0f32 / qfactor
        } else {
            2.0f32 * (1.0 - qfactor / 100.0)
        };

        let (power, alpha_q) = if qfactor <= T0 {
            (1.0f32, ALPHA_T0)
        } else if qfactor < T1 {
            let mut p = m_q.ln() - M_T1.ln();
            p /= M_T0.ln() - M_T1.ln();
            (p, ALPHA_T1 * (ALPHA_T0 / ALPHA_T1).powf(p))
        } else {
            (0.0, ALPHA_T1)
        };
        // ldexp(1.0f, -(int)bit_depth)
        let eps = 0.5f32.sqrt() * ldexp_f32(1.0, -(bit_depth as i32));
        (alpha_q * m_q + eps, power)
    }

    fn ldexp_f32(x: f32, e: i32) -> f32 {
        (x as f64 * 2f64.powi(e)) as f32
    }

    const CB420: [f32; 19] = [
        0.2724, 0.5128, 0.5128, 0.6692, 0.9382, 0.9382, 1.0888, 1.3046, 1.3046, 1.4156, 1.5594,
        1.5594, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0,
    ];
    const CR420: [f32; 19] = [
        0.5196, 0.8260, 0.8260, 1.0080, 1.2928, 1.2928, 1.4440, 1.6508, 1.6508, 1.7538, 1.8848,
        1.8848, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0,
    ];
    const CB422: [f32; 19] = [
        0.1220, 0.1220, 0.3626, 0.3626, 0.3626, 0.6634, 0.6634, 0.6634, 0.9225, 0.9225, 0.9225,
        1.1027, 1.1027, 1.1027, 1.4142, 1.4142, 1.4142, 1.4142, 1.4142,
    ];
    const CR422: [f32; 19] = [
        0.2595, 0.2595, 0.5841, 0.5841, 0.5841, 0.9141, 0.9141, 0.9141, 1.1673, 1.1673, 1.1673,
        1.3328, 1.3328, 1.3328, 1.4142, 1.4142, 1.4142, 1.4142, 1.4142,
    ];
    const CB444: [f32; 19] = [
        0.0263, 0.0863, 0.0863, 0.1362, 0.2564, 0.2564, 0.3346, 0.4691, 0.4691, 0.5444, 0.6523,
        0.6523, 0.7078, 0.7797, 0.7797, 1.0, 1.0, 1.0, 1.0,
    ];
    const CR444: [f32; 19] = [
        0.0773, 0.1835, 0.1835, 0.2598, 0.4130, 0.4130, 0.5040, 0.6464, 0.6464, 0.7220, 0.8254,
        0.8254, 0.8769, 0.9424, 0.9424, 1.0, 1.0, 1.0, 1.0,
    ];
    const Y: [f32; 19] = [
        0.0901, 0.2758, 0.2758, 0.7018, 0.8378, 0.8378, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
        1.0, 1.0, 1.0, 1.0, 1.0,
    ];
    const NO_WEIGHTS: [f32; 19] = [1.0; 19];
}
