// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.
//
// Golden-vector generator for src/transform: calls OpenJPH's generic
// (gen_*) wavelet, colour and code-block kernels on deterministic
// pseudo-random inputs and dumps inputs and outputs to binary files.
// Usage: kernels_dump <output directory>.  See tests/data/kernels/README.md
// for the file format.

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <climits>
#include <cmath>
#include <string>
#include <vector>
#include <type_traits>

#include "ojph_arch.h"
#include "ojph_defs.h"
#include "ojph_mem.h"
#include "ojph_params.h"
#include "../codestream/ojph_params_local.h"
#include "../transform/ojph_transform_local.h"
#include "../transform/ojph_colour_local.h"

namespace ojph {
  namespace local {
    // ojph_codestream_gen.cpp has no header; declared as in
    // ojph_codeblock_fun.cpp.
    void gen_mem_clear(void* addr, size_t count);
    ui32 gen_find_max_val32(ui32* address);
    ui64 gen_find_max_val64(ui64* address);
    void gen_rev_tx_to_cb32(const void *sp, ui32 *dp, ui32 K_max,
                            float delta_inv, ui32 count, ui32* max_val);
    void gen_irv_tx_to_cb32(const void *sp, ui32 *dp, ui32 K_max,
                            float delta_inv, ui32 count, ui32* max_val);
    void gen_rev_tx_to_cb64(const void *sp, ui64 *dp, ui32 K_max,
                            float delta_inv, ui32 count, ui64* max_val);
    void gen_rev_tx_from_cb32(const ui32 *sp, void *dp, ui32 K_max,
                              float delta, ui32 count);
    void gen_irv_tx_from_cb32(const ui32 *sp, void *dp, ui32 K_max,
                              float delta, ui32 count);
    void gen_rev_tx_from_cb64(const ui64 *sp, void *dp, ui32 K_max,
                              float delta, ui32 count);
    void gen_irv_tx_from_cb64(const ui64 *sp, void *dp, ui32 K_max,
                              float delta, ui32 count);
  }
}

using namespace ojph;
using namespace ojph::local;

//////////////////////////////////////////////////////////////////////////////
// Output: a file is a sequence of records; a record is
//   u32 field_count, then per field: u8 tag, u32 element_count, elements
// all little endian.  Tags: 1 i32, 2 i64, 3 f32 (bit pattern), 4 u32,
// 5 u64, 6 u16.
//////////////////////////////////////////////////////////////////////////////
enum Tag : uint8_t { T_I32 = 1, T_I64 = 2, T_F32 = 3, T_U32 = 4, T_U64 = 5,
                     T_U16 = 6 };
template<class T> struct TagOf;
template<> struct TagOf<int32_t>  { static const uint8_t v = T_I32; };
template<> struct TagOf<int64_t>  { static const uint8_t v = T_I64; };
template<> struct TagOf<float>    { static const uint8_t v = T_F32; };
template<> struct TagOf<uint32_t> { static const uint8_t v = T_U32; };
template<> struct TagOf<uint64_t> { static const uint8_t v = T_U64; };
template<> struct TagOf<uint16_t> { static const uint8_t v = T_U16; };

struct Writer {
  FILE* f = nullptr;
  std::vector<uint8_t> rec;
  uint32_t nfields = 0;
  size_t records = 0;
  void open(const std::string& path) {
    f = fopen(path.c_str(), "wb");
    if (!f) { perror(path.c_str()); exit(1); }
  }
  void close() { fclose(f); }
  void put(const void* p, size_t n) {
    const uint8_t* b = (const uint8_t*)p;
    rec.insert(rec.end(), b, b + n);
  }
  void begin() { rec.clear(); nfields = 0; }
  template<class T> void field(const T* p, size_t n) {
    uint8_t tag = TagOf<T>::v;
    uint32_t c = (uint32_t)n;
    put(&tag, 1); put(&c, 4); put(p, n * sizeof(T)); // host is little endian
    ++nfields;
  }
  template<class T> void field(const std::vector<T>& v)
  { field(v.data(), v.size()); }
  template<class T> void scalars(std::initializer_list<T> l)
  { std::vector<T> v(l); field(v); }
  void end() {
    fwrite(&nfields, 4, 1, f);
    fwrite(rec.data(), 1, rec.size(), f);
    ++records;
  }
};

//////////////////////////////////////////////////////////////////////////////
// Deterministic pseudo-random numbers (splitmix64)
//////////////////////////////////////////////////////////////////////////////
static uint64_t rng_state = 0x243F6A8885A308D3ULL;
static uint64_t rnd() {
  uint64_t z = (rng_state += 0x9E3779B97F4A7C15ULL);
  z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
  z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
  return z ^ (z >> 31);
}
static uint32_t rnd_below(uint32_t n) { return (uint32_t)(rnd() % n); }

// mode 0: moderate values; 1: full range and extremes
static int32_t rnd_i32(int mode, int bits = 20) {
  if (mode == 0) {
    int64_t r = (int64_t)(rnd() & ((1ULL << bits) - 1));
    return (int32_t)(r - (int64_t)(1ULL << (bits - 1)));
  }
  static const int32_t ex[] = { INT32_MIN, INT32_MAX, 0, -1, 1,
    INT32_MIN + 1, INT32_MAX - 1, INT32_MIN / 2, INT32_MAX / 2 };
  if (rnd_below(3) == 0) return ex[rnd_below(9)];
  return (int32_t)(uint32_t)rnd();
}
static int64_t rnd_i64(int mode, int bits = 40) {
  if (mode == 0) {
    int64_t r = (int64_t)(rnd() & ((1ULL << bits) - 1));
    return r - (int64_t)(1ULL << (bits - 1));
  }
  static const int64_t ex[] = { INT64_MIN, INT64_MAX, 0, -1, 1,
    INT64_MIN + 1, INT64_MAX - 1, INT32_MIN, INT32_MAX };
  if (rnd_below(3) == 0) return ex[rnd_below(9)];
  return (int64_t)rnd();
}
static float bits_f32(uint32_t u) { float f; memcpy(&f, &u, 4); return f; }
// mode 0: [-0.5, 0.5); 1: also huge values, infinities, NaN, denormals
static float rnd_f32(int mode) {
  float f = (float)(rnd() >> 40) * (1.0f / 16777216.0f) - 0.5f;
  if (mode == 0) return f;
  switch (rnd_below(10)) {
    case 0: return bits_f32((uint32_t)rnd());          // any bit pattern
    case 1: return f * 1e10f;
    case 2: return INFINITY;
    case 3: return -INFINITY;
    case 4: return NAN;
    case 5: return -0.0f;
    case 6: return f * 4294967296.0f;                  // around 2^31
    case 7: return bits_f32(1u + rnd_below(0x7FFFFF)); // denormal
    case 8: return rnd_below(2) ? 0.5f : -0.5f;       // range limits
    default: return f;
  }
}

//////////////////////////////////////////////////////////////////////////////
// Line buffers with one pre-sample and one post-sample, like LineBuf in the
// Rust port with pre_size = 1: logical sample i at vector index 1 + i.
//////////////////////////////////////////////////////////////////////////////
const ui32 PRE = 1;
template<class T> struct Line {
  std::vector<T> v;
  line_buf lb;
  explicit Line(size_t n) {
    v.assign(n + 1 + PRE, T(0));
    for (size_t i = 0; i < v.size(); ++i)
      v[i] = (T)(0x5A + 3 * (int)i); // recognisable filler
    lb.size = n + 1;
    lb.pre_size = PRE;
    lb.flags = sizeof(T) == 8 ? line_buf::LFT_64BIT | line_buf::LFT_INTEGER
             : (std::is_same<T, float>::value ? line_buf::LFT_32BIT
                : line_buf::LFT_32BIT | line_buf::LFT_INTEGER);
    lb.p = v.data() + PRE;
  }
  T* data() { return v.data() + PRE; }
};

template<class T> static T rnd_val(int mode);
template<> int32_t rnd_val<int32_t>(int mode) { return rnd_i32(mode); }
template<> int64_t rnd_val<int64_t>(int mode) { return rnd_i64(mode); }
template<> float   rnd_val<float>(int mode)   { return rnd_f32(mode); }

template<class T> static void fill(Line<T>& l, size_t n, int mode) {
  for (size_t i = 0; i < n; ++i) l.data()[i] = rnd_val<T>(mode);
}

//////////////////////////////////////////////////////////////////////////////
// Kernels
//////////////////////////////////////////////////////////////////////////////
struct RevStep { int a, b, e; };

// Returns an atk object from param_atk::get_atk (1: 5/3, 0: 9/7), with its
// steps optionally overwritten (the atk keeps its number of steps).
static param_atk* make_atk(int index, const std::vector<RevStep>& rev) {
  param_atk* base = new param_atk; // leaked deliberately
  param_atk* atk = base->get_atk(index);
  for (size_t i = 0; i < rev.size(); ++i) {
    lifting_step* s = const_cast<lifting_step*>(atk->get_step((ui32)i));
    s->rev.Aatk = (si16)rev[i].a;
    s->rev.Batk = (si16)rev[i].b;
    s->rev.Eatk = (ui8)rev[i].e;
  }
  return atk;
}

static void write_steps(Writer& w, const param_atk* atk) {
  if (atk->is_reversible()) {
    std::vector<int32_t> s;
    for (ui32 i = 0; i < atk->get_num_steps(); ++i) {
      const lifting_step* st = atk->get_step(i);
      s.push_back(st->rev.Aatk); s.push_back(st->rev.Batk);
      s.push_back(st->rev.Eatk);
    }
    w.field(s);
  } else {
    std::vector<float> s;
    s.push_back(atk->get_K());
    for (ui32 i = 0; i < atk->get_num_steps(); ++i)
      s.push_back(atk->get_step(i)->irv.Aatk);
    w.field(s);
  }
}

// kind: 0 rev 32-bit, 1 rev 64-bit, 2 irv
template<class T>
static void horz_case(Writer& w, int kind, param_atk* atk, ui32 width,
                      bool even, int mode, bool ana) {
  ui32 lw = (width + (even ? 1 : 0)) >> 1, hw = (width + (even ? 0 : 1)) >> 1;
  if (width <= 1) { lw = hw = 1; } // C++ touches sample 0 of one band
  Line<T> a(ana ? lw : width), b(ana ? hw : lw), c(ana ? width : hw);
  // ana: a = ldst, b = hdst, c = src; syn: a = dst, b = lsrc, c = hsrc
  if (ana) fill(c, width == 0 ? 1 : width, mode);
  else { fill(b, lw, mode); fill(c, hw, mode); }
  w.begin();
  w.scalars<uint32_t>({ (uint32_t)kind, width, even ? 1u : 0u, ana ? 1u : 0u,
                        (uint32_t)mode });
  write_steps(w, atk);
  w.field(a.v); w.field(b.v); w.field(c.v);
  if (kind == 2) {
    if (ana) gen_irv_horz_ana(atk, &a.lb, &b.lb, &c.lb, width, even);
    else     gen_irv_horz_syn(atk, &a.lb, &b.lb, &c.lb, width, even);
  } else {
    if (ana) gen_rev_horz_ana(atk, &a.lb, &b.lb, &c.lb, width, even);
    else     gen_rev_horz_syn(atk, &a.lb, &b.lb, &c.lb, width, even);
  }
  w.field(a.v); w.field(b.v); w.field(c.v);
  w.end();
}

static void dump_wavelet_horz(const std::string& dir) {
  Writer w; w.open(dir + "/wavelet_horz.bin");
  std::vector<ui32> widths;
  for (ui32 i = 0; i <= 20; ++i) widths.push_back(i);
  for (ui32 i : { 31u, 32u, 33u, 64u, 65u }) widths.push_back(i);
  std::vector<ui32> few = { 1, 2, 3, 4, 5, 9, 16, 17 };

  param_atk* rev53 = make_atk(1, {});
  param_atk* irv97 = make_atk(0, {});
  // two general reversible kernels exercising the non-5/3 branches
  param_atk* revg1 = make_atk(1, { { -1, 3, 2 }, { 3, -5, 3 } });
  param_atk* revg2 = make_atk(1, { { 1, 0, 0 }, { -2, 1, 1 } });

  for (int ana = 0; ana < 2; ++ana)
    for (int ev = 0; ev < 2; ++ev) {
      for (ui32 width : widths) {
        horz_case<int32_t>(w, 0, rev53, width, ev, 0, ana);
        horz_case<float>(w, 2, irv97, width, ev, 0, ana);
      }
      for (ui32 width : few) {
        horz_case<int32_t>(w, 0, rev53, width, ev, 1, ana);
        horz_case<int64_t>(w, 1, rev53, width, ev, 0, ana);
        horz_case<int64_t>(w, 1, rev53, width, ev, 1, ana);
        horz_case<float>(w, 2, irv97, width, ev, 1, ana);
        horz_case<int32_t>(w, 0, revg1, width, ev, 0, ana);
        horz_case<int32_t>(w, 0, revg2, width, ev, 1, ana);
        horz_case<int64_t>(w, 1, revg1, width, ev, 1, ana);
        horz_case<int64_t>(w, 1, revg2, width, ev, 0, ana);
      }
    }
  printf("wavelet_horz: %zu records\n", w.records);
  w.close();
}

// kind: 0 rev 32, 1 rev 64, 2 irv step, 3 irv times K
template<class T>
static void vert_case(Writer& w, int kind, const lifting_step& s, float K,
                      ui32 repeat, bool synthesis, bool alias, int mode) {
  Line<T> sig(repeat), other(repeat), aug(repeat);
  fill(sig, repeat, mode); fill(other, repeat, mode); fill(aug, repeat, mode);
  w.begin();
  w.scalars<uint32_t>({ (uint32_t)kind, repeat, synthesis ? 1u : 0u,
                        alias ? 1u : 0u, (uint32_t)mode });
  if (kind <= 1) w.scalars<int32_t>({ s.rev.Aatk, s.rev.Batk, s.rev.Eatk });
  else w.scalars<float>({ kind == 2 ? s.irv.Aatk : K });
  w.field(sig.v); w.field(other.v); w.field(aug.v);
  const line_buf* o = alias ? &sig.lb : &other.lb;
  if (kind <= 1) gen_rev_vert_step(&s, &sig.lb, o, &aug.lb, repeat, synthesis);
  else if (kind == 2)
    gen_irv_vert_step(&s, &sig.lb, o, &aug.lb, repeat, synthesis);
  else gen_irv_vert_times_K(K, &aug.lb, repeat);
  w.field(aug.v);
  w.end();
}

static void dump_wavelet_vert(const std::string& dir) {
  Writer w; w.open(dir + "/wavelet_vert.bin");
  std::vector<RevStep> rs = { { 1, 2, 2 }, { -1, 1, 1 }, { -1, 3, 2 },
    { 3, -5, 3 }, { 1, 0, 0 }, { -2, 1, 1 }, { 7, 100, 5 } };
  std::vector<float> irs = { 0.443506852043971f, 0.882911075530934f,
    -0.052980118572961f, -1.586134342059924f, 0.5f };
  std::vector<ui32> reps = { 0, 1, 3, 13 };
  for (ui32 rep : reps)
    for (int syn = 0; syn < 2; ++syn)
      for (int alias = 0; alias < 2; ++alias)
        for (int mode = 0; mode < 2 - alias; ++mode) {
          for (const RevStep& r : rs) {
            lifting_step s; memset(&s, 0, sizeof(s));
            s.rev.Aatk = (si16)r.a; s.rev.Batk = (si16)r.b;
            s.rev.Eatk = (ui8)r.e;
            vert_case<int32_t>(w, 0, s, 0, rep, syn, alias, mode);
            vert_case<int64_t>(w, 1, s, 0, rep, syn, alias, mode);
          }
          for (float a : irs) {
            lifting_step s; memset(&s, 0, sizeof(s));
            s.irv.Aatk = a;
            vert_case<float>(w, 2, s, 0, rep, syn, alias, mode);
          }
          if (syn == 0 && alias == 0) {
            lifting_step s; memset(&s, 0, sizeof(s));
            vert_case<float>(w, 3, s, 1.230174104914001f, rep, 0, 0, mode);
            vert_case<float>(w, 3, s, 1.0f / 1.230174104914001f, rep, 0, 0,
                             mode);
          }
        }
  printf("wavelet_vert: %zu records\n", w.records);
  w.close();
}

//////////////////////////////////////////////////////////////////////////////
// Colour
//////////////////////////////////////////////////////////////////////////////
// func ids
enum { F_REV_CONVERT = 0, F_REV_CONVERT_NLT3 = 1, F_IRV_TO_INT = 2,
       F_IRV_TO_INT_NLT3 = 3, F_IRV_TO_FLOAT = 4, F_IRV_TO_FLOAT_NLT3 = 5,
       F_RCT_FWD = 6, F_RCT_BWD = 7, F_ICT_FWD = 8, F_ICT_BWD = 9 };

// rev_convert[_nlt_type3]; combo 0: 32->32, 1: 32->64, 2: 64->32
template<class S, class D>
static void rev_convert_case(Writer& w, int func, int combo, si64 shift,
                             ui32 so, ui32 doff, ui32 width, int mode) {
  Line<S> src(so + width); Line<D> dst(doff + width);
  fill(src, so + width, mode);
  w.begin();
  w.scalars<uint32_t>({ (uint32_t)func, (uint32_t)combo, so, doff, width,
                        (uint32_t)mode });
  w.scalars<int64_t>({ shift });
  w.field(src.v); w.field(dst.v);
  if (func == F_REV_CONVERT)
    gen_rev_convert(&src.lb, so, &dst.lb, doff, shift, width);
  else
    gen_rev_convert_nlt_type3(&src.lb, so, &dst.lb, doff, shift, width);
  w.field(dst.v);
  w.end();
}

static void irv_conv_case(Writer& w, int func, ui32 bd, bool is_signed,
                          ui32 off, ui32 width, int mode) {
  bool to_int = func == F_IRV_TO_INT || func == F_IRV_TO_INT_NLT3;
  w.begin();
  w.scalars<uint32_t>({ (uint32_t)func, bd, is_signed ? 1u : 0u, off, width,
                        (uint32_t)mode });
  if (to_int) {
    Line<float> src(width); Line<int32_t> dst(off + width);
    fill(src, width, mode);
    w.field(src.v); w.field(dst.v);
    if (func == F_IRV_TO_INT)
      gen_irv_convert_to_integer(&src.lb, &dst.lb, off, bd, is_signed, width);
    else
      gen_irv_convert_to_integer_nlt_type3(&src.lb, &dst.lb, off, bd,
                                           is_signed, width);
    w.field(dst.v);
  } else {
    Line<int32_t> src(off + width); Line<float> dst(width);
    for (ui32 i = 0; i < off + width; ++i) {
      int32_t v;
      if (mode == 0) { // values of bd bits
        uint64_t r = rnd() & ((1ULL << bd) - 1);
        v = is_signed ? (int32_t)((int64_t)r - (int64_t)(1ULL << (bd - 1)))
                      : (int32_t)(uint32_t)r;
      } else v = rnd_i32(1);
      src.data()[i] = v;
    }
    w.field(src.v); w.field(dst.v);
    if (func == F_IRV_TO_FLOAT)
      gen_irv_convert_to_float(&src.lb, off, &dst.lb, bd, is_signed, width);
    else
      gen_irv_convert_to_float_nlt_type3(&src.lb, off, &dst.lb, bd,
                                         is_signed, width);
    w.field(dst.v);
  }
  w.end();
}

// wide: rct with 64-bit y/cb/cr
static void rct_case(Writer& w, int func, bool wide, ui32 repeat, int mode) {
  w.begin();
  w.scalars<uint32_t>({ (uint32_t)func, wide ? 1u : 0u, repeat,
                        (uint32_t)mode });
  Line<int32_t> r(repeat), g(repeat), b(repeat);
  if (!wide) {
    Line<int32_t> y(repeat), cb(repeat), cr(repeat);
    if (func == F_RCT_FWD) {
      fill(r, repeat, mode); fill(g, repeat, mode); fill(b, repeat, mode);
    } else {
      fill(y, repeat, mode); fill(cb, repeat, mode); fill(cr, repeat, mode);
    }
    w.field(r.v); w.field(g.v); w.field(b.v);
    w.field(y.v); w.field(cb.v); w.field(cr.v);
    if (func == F_RCT_FWD)
      gen_rct_forward(&r.lb, &g.lb, &b.lb, &y.lb, &cb.lb, &cr.lb, repeat);
    else
      gen_rct_backward(&y.lb, &cb.lb, &cr.lb, &r.lb, &g.lb, &b.lb, repeat);
    w.field(r.v); w.field(g.v); w.field(b.v);
    w.field(y.v); w.field(cb.v); w.field(cr.v);
  } else {
    Line<int64_t> y(repeat), cb(repeat), cr(repeat);
    if (func == F_RCT_FWD) {
      fill(r, repeat, mode); fill(g, repeat, mode); fill(b, repeat, mode);
    } else {
      fill(y, repeat, mode); fill(cb, repeat, mode); fill(cr, repeat, mode);
    }
    w.field(r.v); w.field(g.v); w.field(b.v);
    w.field(y.v); w.field(cb.v); w.field(cr.v);
    if (func == F_RCT_FWD)
      gen_rct_forward(&r.lb, &g.lb, &b.lb, &y.lb, &cb.lb, &cr.lb, repeat);
    else
      gen_rct_backward(&y.lb, &cb.lb, &cr.lb, &r.lb, &g.lb, &b.lb, repeat);
    w.field(r.v); w.field(g.v); w.field(b.v);
    w.field(y.v); w.field(cb.v); w.field(cr.v);
  }
  w.end();
}

static void ict_case(Writer& w, int func, ui32 repeat, int mode) {
  w.begin();
  w.scalars<uint32_t>({ (uint32_t)func, 0u, repeat, (uint32_t)mode });
  std::vector<float> in[3], out[3];
  for (int c = 0; c < 3; ++c) {
    in[c].resize(repeat); out[c].assign(repeat, 0.0f);
    for (ui32 i = 0; i < repeat; ++i) in[c][i] = rnd_f32(mode);
  }
  for (int c = 0; c < 3; ++c) w.field(in[c]);
  if (func == F_ICT_FWD)
    gen_ict_forward(in[0].data(), in[1].data(), in[2].data(),
                    out[0].data(), out[1].data(), out[2].data(), repeat);
  else
    gen_ict_backward(in[0].data(), in[1].data(), in[2].data(),
                     out[0].data(), out[1].data(), out[2].data(), repeat);
  for (int c = 0; c < 3; ++c) w.field(out[c]);
  w.end();
}

static void dump_colour(const std::string& dir) {
  Writer w; w.open(dir + "/colour.bin");
  // record 0: the CT_CNST constants
  w.begin();
  w.scalars<float>({ CT_CNST::ALPHA_RF, CT_CNST::ALPHA_GF, CT_CNST::ALPHA_BF,
    CT_CNST::BETA_CbF, CT_CNST::BETA_CrF, CT_CNST::GAMMA_CR2R,
    CT_CNST::GAMMA_CB2B, CT_CNST::GAMMA_CR2G, CT_CNST::GAMMA_CB2G });
  w.end();

  std::vector<si64> shifts = { 0, 1, -1, 128, 1 << 15, (si64)1 << 31,
    -((si64)1 << 31), (si64)1 << 40, INT64_MIN, INT64_MAX };
  for (int func = F_REV_CONVERT; func <= F_REV_CONVERT_NLT3; ++func)
    for (si64 sh : shifts)
      for (int mode = 0; mode < 2; ++mode) {
        ui32 so = rnd_below(4), doff = rnd_below(4), width = 1 + rnd_below(12);
        rev_convert_case<int32_t, int32_t>(w, func, 0, sh, so, doff, width,
                                           mode);
        rev_convert_case<int32_t, int64_t>(w, func, 1, sh, so, doff, width,
                                           mode);
        rev_convert_case<int64_t, int32_t>(w, func, 2, sh, so, doff, width,
                                           mode);
      }
  for (int func = F_IRV_TO_INT; func <= F_IRV_TO_FLOAT_NLT3; ++func)
    for (ui32 bd = 1; bd <= 32; ++bd)
      for (int s = 0; s < 2; ++s)
        for (int mode = 0; mode < 2; ++mode)
          if (mode == 0 || bd % 4 == 0 || bd < 3 || bd == 31)
              irv_conv_case(w, func, bd, s, rnd_below(3), 6 + 2 * mode, mode);
  for (int func = F_RCT_FWD; func <= F_RCT_BWD; ++func)
    for (ui32 rep : { 0u, 1u, 5u, 16u, 33u })
      for (int mode = 0; mode < 2; ++mode) {
        rct_case(w, func, false, rep, mode);
        rct_case(w, func, true, rep, mode);
      }
  for (int func = F_ICT_FWD; func <= F_ICT_BWD; ++func)
    for (ui32 rep : { 0u, 1u, 7u, 32u })
      for (int mode = 0; mode < 2; ++mode)
        ict_case(w, func, rep, mode);
  printf("colour: %zu records\n", w.records);
  w.close();
}

//////////////////////////////////////////////////////////////////////////////
// LUT-style nonlinearities (types 2 and 4), with hand-filled nlt_rec objects
// holding synthetic monotone tables.
//////////////////////////////////////////////////////////////////////////////
static std::vector<float> monotone(size_t n, float lo, float hi, bool flat) {
  std::vector<float> v(n);
  float acc = 0, tot = 0;
  std::vector<float> inc(n);
  for (size_t i = 0; i < n; ++i) {
    inc[i] = (flat && rnd_below(4) == 0) ? 0.0f
             : (float)(1 + rnd_below(1000));
    tot += inc[i];
  }
  for (size_t i = 0; i < n; ++i) {
    v[i] = lo + (hi - lo) * (acc / tot);
    acc += inc[i];
  }
  return v;
}

// variant: 0 to integer, 1 to float approx, 2 to float precise one step,
// 3 to float precise search
static void nlt_case(Writer& w, int variant, ui8 type, ui32 bd, bool sgn,
                     int mode) {
  const ui32 num_points = 9 + rnd_below(12);
  const ui32 enc_points = variant == 2 ? 4 * num_points
                         : (variant == 3 ? num_points / 3 : 2 * num_points);
  const ui32 width = 16, off = rnd_below(3);

  nlt_rec rec;
  rec.Tnlt = type;
  rec.BDnlt = (ui8)((bd - 1) | (sgn ? 0x80 : 0));
  rec.num_points = num_points;
  rec.fd_min = 0.0f; rec.fd_max = 1.0f;
  rec.delta = 1.0f / (float)(num_points - 1);
  rec.inv_delta = (float)(num_points - 1);
  rec.ft_min = 0.0f; rec.ft_max = 1.0f;
  rec.approx_encoding_nlt = variant == 1;
  rec.precise_max_steps = variant == 2 ? 1 : (variant == 3 ? 4 : 0);

  std::vector<float> dec_points, approx, plut;
  std::vector<ui16> indices;
  if (variant == 0) {
    dec_points = monotone(num_points + 1, 0.0f, 1.0f, false);
    rec.multiplier = (float)((1ULL << bd) - 1);
  } else {
    rec.multiplier = 1.0f / (float)((1ULL << bd) - 1);
    // for the to-float kernels inv_delta indexes the encoder tables
    rec.inv_delta = (float)(enc_points - 1);
    rec.delta = 1.0f / (float)(num_points - 1);
    approx = monotone(enc_points + 2, 0.0f, 1.0f, false);
    indices.resize(enc_points + 1);
    for (ui32 i = 0; i <= enc_points; ++i)
      indices[i] = (ui16)ojph_min((ui32)((uint64_t)i * (num_points - 1)
                                         / enc_points), num_points - 3);
    plut = monotone(num_points + 3, 0.0f, 1.0f, true);
  }
  rec.dec_points = dec_points.empty() ? nullptr : dec_points.data();
  rec.approx_enc_points = approx.empty() ? nullptr : approx.data();
  rec.precise_enc_dec_indices = indices.empty() ? nullptr : indices.data();
  rec.precise_dec_lut = plut.empty() ? nullptr : plut.data();

  w.begin();
  w.scalars<uint32_t>({ (uint32_t)variant, type, bd, sgn ? 1u : 0u,
    rec.approx_encoding_nlt ? 1u : 0u, rec.precise_max_steps, num_points,
    width, off, (uint32_t)mode });
  w.scalars<float>({ rec.multiplier, rec.fd_min, rec.fd_max, rec.delta,
    rec.inv_delta, rec.ft_min, rec.ft_max });
  w.field(dec_points); w.field(approx); w.field(indices); w.field(plut);
  if (variant == 0) {
    Line<float> src(width); Line<int32_t> dst(off + width);
    fill(src, width, mode);
    w.field(src.v); w.field(dst.v);
    // bit_depth and is_signed arguments are unused; pass different values
    gen_irv_convert_to_integer_nlt(&src.lb, &dst.lb, off, 7, !sgn, width,
                                   &rec);
    w.field(dst.v);
  } else {
    Line<int32_t> src(off + width); Line<float> dst(width);
    for (ui32 i = 0; i < off + width; ++i) {
      int32_t v;
      if (mode == 0) {
        uint64_t r = rnd() & ((1ULL << bd) - 1);
        v = sgn ? (int32_t)((int64_t)r - (int64_t)(1ULL << (bd - 1)))
                : (int32_t)(uint32_t)r;
      } else v = rnd_i32(1);
      src.data()[i] = v;
    }
    w.field(src.v); w.field(dst.v);
    gen_irv_convert_to_float_nlt(&src.lb, off, &dst.lb, 7, !sgn, width, &rec);
    w.field(dst.v);
  }
  w.end();
  // the tables are owned by the vectors; keep nlt_rec from freeing them
  rec.points_store = nullptr;
}

static void dump_nlt(const std::string& dir) {
  Writer w; w.open(dir + "/nlt.bin");
  for (int variant = 0; variant < 4; ++variant)
    for (ui8 type : { (ui8)2, (ui8)4 })
      for (ui32 bd : { 1u, 8u, 16u, 31u })
        for (int sgn = 0; sgn < 2; ++sgn)
          for (int mode = 0; mode < 2; ++mode)
            nlt_case(w, variant, type, bd, sgn, mode);
  printf("nlt: %zu records\n", w.records);
  w.close();
}

//////////////////////////////////////////////////////////////////////////////
// Code-block conversions
//////////////////////////////////////////////////////////////////////////////
enum { C_REV_TO32 = 0, C_REV_TO64 = 1, C_IRV_TO32 = 2, C_REV_FROM32 = 3,
       C_REV_FROM64 = 4, C_IRV_FROM32 = 5, C_IRV_FROM64 = 6 };

static void cb_case(Writer& w, int func, ui32 k_max, float delta,
                    ui32 count, int mode) {
  w.begin();
  w.scalars<uint32_t>({ (uint32_t)func, k_max, count, (uint32_t)mode });
  w.scalars<float>({ delta });
  switch (func) {
    case C_REV_TO32: {
      std::vector<int32_t> sp(count);
      for (auto& v : sp)
        v = mode == 0 ? rnd_i32(0, ojph_max(1u, ojph_min(k_max, 31u)))
                      : rnd_i32(1);
      std::vector<uint32_t> dp(count, 0xDEADBEEF);
      uint32_t mv[1] = { mode ? (uint32_t)rnd() & 0x00F0F000u : 0u };
      w.field(sp); w.field(mv, 1);
      gen_rev_tx_to_cb32(sp.data(), dp.data(), k_max, delta, count, mv);
      w.field(dp); w.field(mv, 1);
      break;
    }
    case C_REV_TO64: {
      std::vector<int64_t> sp(count);
      for (auto& v : sp)
        v = mode == 0 ? rnd_i64(0, ojph_max(1u, ojph_min(k_max, 63u)))
                      : rnd_i64(1);
      std::vector<uint64_t> dp(count, 0xDEADBEEF);
      uint64_t mv[1] = { mode ? rnd() & 0x00F0F000F0F00000ull : 0u };
      w.field(sp); w.field(mv, 1);
      gen_rev_tx_to_cb64(sp.data(), dp.data(), k_max, delta, count, mv);
      w.field(dp); w.field(mv, 1);
      break;
    }
    case C_IRV_TO32: {
      std::vector<float> sp(count);
      for (auto& v : sp) v = rnd_f32(mode);
      std::vector<uint32_t> dp(count, 0xDEADBEEF);
      uint32_t mv[1] = { mode ? (uint32_t)rnd() & 0x00F0F000u : 0u };
      w.field(sp); w.field(mv, 1);
      gen_irv_tx_to_cb32(sp.data(), dp.data(), k_max, delta, count, mv);
      w.field(dp); w.field(mv, 1);
      break;
    }
    case C_REV_FROM32: case C_IRV_FROM32: {
      std::vector<uint32_t> sp(count);
      for (auto& v : sp) {
        v = (uint32_t)rnd();
        if (mode == 0 && func == C_REV_FROM32)
          v &= 0x80000000u | (0x7FFFFFFFu >> k_max << (31 - k_max)) |
               ((1u << (31 - k_max)) - 1);
      }
      w.field(sp);
      if (func == C_REV_FROM32) {
        std::vector<int32_t> dp(count, 0x5A5A5A5A);
        gen_rev_tx_from_cb32(sp.data(), dp.data(), k_max, delta, count);
        w.field(dp);
      } else {
        std::vector<float> dp(count, 0.0f);
        gen_irv_tx_from_cb32(sp.data(), dp.data(), k_max, delta, count);
        w.field(dp);
      }
      break;
    }
    case C_REV_FROM64: case C_IRV_FROM64: {
      std::vector<uint64_t> sp(count);
      for (auto& v : sp) {
        v = rnd();
        if (mode == 0) v >>= rnd_below(64);
        if (rnd_below(2)) v |= 0x8000000000000000ull;
      }
      w.field(sp);
      if (func == C_REV_FROM64) {
        std::vector<int64_t> dp(count, 0x5A5A5A5A);
        gen_rev_tx_from_cb64(sp.data(), dp.data(), k_max, delta, count);
        w.field(dp);
      } else {
        std::vector<float> dp(count, 0.0f);
        gen_irv_tx_from_cb64(sp.data(), dp.data(), k_max, delta, count);
        w.field(dp);
      }
      break;
    }
  }
  w.end();
}

static void dump_codeblock(const std::string& dir) {
  Writer w; w.open(dir + "/codeblock.bin");
  for (int mode = 0; mode < 2; ++mode) {
    for (ui32 k = 0; k <= 31; k += 1 + 2 * mode) {
      cb_case(w, C_REV_TO32, k, 0.0f, 9, mode);
      cb_case(w, C_REV_FROM32, k, 0.0f, 9, mode);
    }
    for (ui32 k = 0; k <= 63; k += 1 + 2 * mode) {
      cb_case(w, C_REV_TO64, k, 0.0f, 7, mode);
      cb_case(w, C_REV_FROM64, k, 0.0f, 7, mode);
    }
    for (float d : { 1.0f, 0.5f, 3.0f, 1024.0f, 1.0f / 3.0f, 65536.0f * 4096.0f,
                     1e-3f, 0.0f })
      for (ui32 count : { 0u, 1u, 13u }) {
        cb_case(w, C_IRV_TO32, (ui32)rnd_below(32), d, count, mode);
        cb_case(w, C_IRV_FROM32, (ui32)rnd_below(32), d, count, mode);
        cb_case(w, C_IRV_FROM64, (ui32)rnd_below(64), d, count, mode);
      }
  }
  printf("codeblock: %zu records\n", w.records);
  w.close();
}

int main(int argc, char** argv) {
  if (argc != 2) { fprintf(stderr, "usage: %s <out dir>\n", argv[0]); return 1; }
  std::string dir = argv[1];
  dump_wavelet_horz(dir);
  dump_wavelet_vert(dir);
  dump_colour(dir);
  dump_nlt(dir);
  dump_codeblock(dir);
  return 0;
}
