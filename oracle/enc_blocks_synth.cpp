// enc_blocks_synth.cpp: calls OpenJPH's generic HT block encoders directly
// with synthetic code blocks, so that $OJPH_ENC_BLOCK_DUMP receives records
// (written by enc_block_dump.cpp) for inputs that encoding an image never
// produces:
//
//  * all-zero blocks (codeblock::encode skips the encoder for those);
//  * every missing_msbs the encoders support (32-bit: 0 to 29, 64-bit: 0 to
//    61), with magnitudes up to the largest the encoder can code, so the
//    64-bit UVLC extension (u_q > 32) is exercised;
//  * widths of 1021 to 1024 and other edge sizes, strides wider than the
//    block (the padding holds garbage the encoder must not read), sample
//    bits below the coded bit planes, and signed zeros;
//  * blocks larger than JPEG 2000 allows (1024 x 8 and 1024 x 16), on which the
//    encoder's fixed-size buffers fill up and it raises OJPH_ERROR.
//
// Usage: OJPH_ENC_BLOCK_DUMP=out.ojbe enc_blocks_synth
// The output is deterministic (splitmix64 with fixed seeds).

#include <openjph/ojph_defs.h>
#include <openjph/ojph_mem.h>
#include <openjph/ojph_message.h>

#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <exception>
#include <stdexcept>
#include <vector>

extern ojph::ui32 block_dump_last_error;

namespace ojph {
namespace local {
void ojph_encode_codeblock32(ui32* buf, ui32 missing_msbs, ui32 num_passes,
  ui32 width, ui32 height, ui32 stride, ui32* lengths,
  mem_elastic_allocator* elastic, coded_lists*& coded);
void ojph_encode_codeblock64(ui64* buf, ui32 missing_msbs, ui32 num_passes,
  ui32 width, ui32 height, ui32 stride, ui32* lengths,
  mem_elastic_allocator* elastic, coded_lists*& coded);
bool initialize_block_encoder_tables();
}  // namespace local
}  // namespace ojph

using namespace ojph;

namespace {

// Records the error code for enc_block_dump.cpp, then throws like
// OpenJPH's default handler.
class recording_error : public message_error {
 public:
  void operator()(int code, const char*, int, const char*, ...) override {
    block_dump_last_error = (ui32)code;
    throw std::runtime_error("ojph error");
  }
};

struct rng {
  ui64 s;
  ui64 next() {
    ui64 z = (s += 0x9E3779B97F4A7C15ULL);
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
    return z ^ (z >> 31);
  }
  ui64 below(ui64 n) { return n ? next() % n : 0; }
};

enum pattern {
  P_ZERO,    // all zero (some with sign bits set: signed zeros)
  P_CONST,   // one value everywhere
  P_NOISE,   // every bit random: magnitudes over the full coded range
  P_LIMITED, // random magnitudes of at most `bits` coded bit planes
  P_SPARSE,  // mostly zero, a few random samples (long MEL runs)
  P_SMOOTH,  // a diagonal ramp, as after a wavelet on a smooth image
  P_MAX,     // every magnitude all ones (largest E values)
  P_SINGLE,  // one significant sample
  P_STRIPES, // quad rows alternating between zero and noise
  P_COUNT
};

const char* pattern_name[P_COUNT] = {"zero", "const", "noise", "limited",
  "sparse", "smooth", "max", "single", "stripes"};

// Fills a height x stride buffer (padding included) with samples of a
// `bits`-bit encoder (32 or 64) for the given missing_msbs.  Samples are
// sign-magnitude: the sign in the top bit, the magnitude below it, of which
// bits p and up are coded, where p = bits - 2 - missing_msbs.
template <typename T>
void fill(std::vector<T>& b, rng& r, pattern pat, int bits, ui32 mm,
          ui32 width, ui32 height, ui32 stride) {
  const ui32 p = (ui32)bits - 2 - mm;
  const T sign = (T)1 << (bits - 1);
  const T mag_mask = sign - 1;
  ui32 nplanes = (ui32)bits - 1 - p;  // coded planes = mm + 1
  ui32 lim = 1 + (ui32)r.below(nplanes);
  T cval = (T)r.next() & (sign | mag_mask);
  ui32 sx = (ui32)r.below(width), sy = (ui32)r.below(height);
  for (size_t i = 0; i < b.size(); ++i)
    b[i] = (T)(r.next() | 0xA5);  // padding garbage, overwritten in the block
  for (ui32 y = 0; y < height; ++y)
    for (ui32 x = 0; x < width; ++x) {
      T v = 0;
      T rnd = (T)r.next();
      switch (pat) {
        case P_ZERO: v = (r.below(4) == 0) ? sign : 0; break;
        case P_CONST: v = cval; break;
        case P_NOISE: v = rnd; break;
        case P_LIMITED: {
          T m = (T)(rnd & mag_mask) >> (nplanes - lim + p);
          v = (T)(m << p) | (rnd & sign);
          if (r.below(2)) v |= (T)r.next() & (((T)1 << p) - 1);
          break;
        }
        case P_SPARSE:
          v = r.below(40) == 0 ? rnd : (rnd & sign);
          break;
        case P_SMOOTH: {
          T m = (T)((x + y) * 37 + r.below(8));
          if (nplanes < bits - 1 && nplanes < 40)
            m &= ((T)1 << nplanes) - 1;
          v = (T)(m << p) | ((x & 8) ? sign : 0);
          break;
        }
        case P_MAX: v = mag_mask | (rnd & sign); break;
        case P_SINGLE:
          v = (x == sx && y == sy) ? (T)(rnd | ((T)1 << p)) : 0;
          break;
        case P_STRIPES: v = ((y >> 1) & 1) ? rnd : 0; break;
        default: break;
      }
      b[(size_t)y * stride + x] = v;
    }
}

template <typename T, typename F>
void run_case(F enc, mem_elastic_allocator& el, rng& r, pattern pat, int bits,
              ui32 mm, ui32 w, ui32 h, int& ok, int& failed) {
  ui32 stride = w + (r.below(3) == 0 ? 0 : (ui32)r.below(9));
  std::vector<T> b((size_t)stride * ((h + 1) & ~1u));
  fill(b, r, pat, bits, mm, w, h, stride);
  ui32 lengths[2] = {0, 0};
  coded_lists* coded = NULL;
  try {
    enc(b.data(), mm, 1, w, h, stride, lengths, &el, coded);
    ++ok;
  } catch (const std::exception&) {
    ++failed;
    fprintf(stderr, "error 0x%08X: %d-bit %ux%u mm %u %s\n",
            block_dump_last_error, bits, w, h, mm, pattern_name[pat]);
  }
  el.restart();
}

}  // namespace

int main() {
  recording_error handler;
  configure_error(&handler);
  local::initialize_block_encoder_tables();
  mem_elastic_allocator el(1 << 20);
  int ok = 0, failed = 0;

  // Small and edge sizes: every pattern, with random missing_msbs.
  static const ui32 small[][2] = {
    {1, 1}, {1, 2}, {2, 1}, {2, 2}, {3, 3}, {4, 4}, {5, 7}, {7, 5}, {8, 8},
    {6, 3}, {9, 2}, {13, 17}, {16, 16}, {17, 1}, {1, 17}, {33, 31},
    {31, 33}, {64, 3}, {3, 64}};
  // Large and full-width sizes, including widths 1021 to 1024.
  static const ui32 large[][2] = {
    {64, 64}, {128, 32}, {32, 128}, {1024, 4}, {1023, 4}, {1022, 3},
    {1021, 2}, {1024, 1}, {4, 1024}, {1, 1024}, {63, 64}};
  rng r = {0x5EED0001};
  for (int bits = 32; bits <= 64; bits += 32) {
    ui32 mm_max = bits == 32 ? 29 : 61;
    for (const auto& s : small)
      for (int pat = 0; pat < P_COUNT; ++pat) {
        ui32 mm = (ui32)r.below(mm_max + 1);
        if (bits == 32)
          run_case<ui32>(local::ojph_encode_codeblock32, el, r, (pattern)pat,
                         bits, mm, s[0], s[1], ok, failed);
        else
          run_case<ui64>(local::ojph_encode_codeblock64, el, r, (pattern)pat,
                         bits, mm, s[0], s[1], ok, failed);
      }
    // Large blocks: one pattern each (in turn), plus limited-range noise for
    // the widths of 1021 and up, to keep the vectors small.
    static const pattern lp[] = {P_ZERO, P_LIMITED, P_SPARSE, P_SMOOTH};
    int li = bits / 32;
    for (const auto& s : large) {
      const pattern first = lp[li++ % 4];
      for (int k = 0; k < 2; ++k) {
        const pattern pat = k == 0 ? first : P_LIMITED;
        if (k == 1 && (s[0] < 1021 || first == P_LIMITED)) continue;
        ui32 mm = (ui32)r.below(mm_max + 1);
        if (bits == 32)
          run_case<ui32>(local::ojph_encode_codeblock32, el, r, pat, bits,
                         mm, s[0], s[1], ok, failed);
        else
          run_case<ui64>(local::ojph_encode_codeblock64, el, r, pat, bits,
                         mm, s[0], s[1], ok, failed);
      }
    }
    // Every missing_msbs on a 8x8 block of maximal and of noise samples.
    for (ui32 mm = 0; mm <= mm_max; ++mm)
      for (pattern pat : {P_MAX, P_NOISE}) {
        if (bits == 32)
          run_case<ui32>(local::ojph_encode_codeblock32, el, r, pat, bits,
                         mm, 8, 8, ok, failed);
        else
          run_case<ui64>(local::ojph_encode_codeblock64, el, r, pat, bits,
                         mm, 8, 8, ok, failed);
      }
  }

  // Full-size blocks at the top of the range: the 64-bit MagSgn buffer
  // (24030 bytes) cannot hold 4096 samples of 63 bits.
  run_case<ui64>(local::ojph_encode_codeblock64, el, r, P_NOISE, 64, 61, 64,
                 64, ok, failed);
  run_case<ui64>(local::ojph_encode_codeblock64, el, r, P_MAX, 64, 50, 1024,
                 4, ok, failed);
  run_case<ui64>(local::ojph_encode_codeblock64, el, r, P_LIMITED, 64, 40,
                 64, 64, ok, failed);
  run_case<ui32>(local::ojph_encode_codeblock32, el, r, P_MAX, 32, 29, 64, 64,
                 ok, failed);
  // Oversized blocks (the C++ accepts any height): the 32-bit MagSgn buffer
  // (17477 bytes) fills at 1024 x 8; the VLC and MEL buffers at 1024 x 16,
  // the smallest height, in steps of four rows, at which they do here.
  run_case<ui32>(local::ojph_encode_codeblock32, el, r, P_NOISE, 32, 29,
                 1024, 8, ok, failed);
  run_case<ui32>(local::ojph_encode_codeblock32, el, r, P_SMOOTH, 32, 4,
                 1024, 16, ok, failed);
  run_case<ui32>(local::ojph_encode_codeblock32, el, r, P_SPARSE, 32, 10,
                 1024, 16, ok, failed);
  run_case<ui64>(local::ojph_encode_codeblock64, el, r, P_SPARSE, 64, 3,
                 1024, 16, ok, failed);
  fprintf(stderr, "enc_blocks_synth: %d encoded, %d raised errors\n", ok,
          failed);
  return 0;
}
