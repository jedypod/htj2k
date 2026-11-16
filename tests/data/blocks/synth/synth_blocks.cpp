// synth_blocks.cpp: makes synthetic HT code-block decoder vectors (OJBH
// records, see ../README.md) by calling OpenJPH 0.33.0's generic
// ojph_decode_codeblock32 / ojph_decode_codeblock64 directly.
//
// Seeds are real cleanup passes taken from OJBK record files (as written by
// oracle/block_dump.cpp). From each seed and from pure noise it derives
// calls that OpenJPH's own encoder never produces: SigProp/MagRef passes
// (arbitrary refinement bytes appended), stripe-causal mode, bit flips,
// truncations, other missing_msbs and smaller geometries.
//
// Every call is made at the four byte alignments of coded_data, with
// different garbage around it; a case whose results differ between
// alignments is reported and dropped (none are expected). The decoded
// buffer is (height + 4) rows of `stride` samples, zero-filled, and the
// record stores the FNV-1a-64 of the whole buffer after the call.
//
// Usage: synth_blocks MODE OUT SEED.ojbk...  (OUT: synth.ojbh)
//   MODE "list": print "CASE n" to stderr before each call (for a
//                sanitizer build, to find cases with undefined behaviour);
//   MODE "write[:n,n,...]": write OUT, skipping the listed case numbers.
// See build.sh.

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <set>
#include <string>
#include <vector>

typedef uint8_t ui8;
typedef uint32_t ui32;
typedef uint64_t ui64;

namespace ojph {
namespace local {
bool ojph_decode_codeblock32(ui8* coded_data, ui32* decoded_data,
  ui32 missing_msbs, ui32 num_passes, ui32 lengths1, ui32 lengths2,
  ui32 width, ui32 height, ui32 stride, bool stripe_causal);
bool ojph_decode_codeblock64(ui8* coded_data, ui64* decoded_data,
  ui32 missing_msbs, ui32 num_passes, ui32 lengths1, ui32 lengths2,
  ui32 width, ui32 height, ui32 stride, bool stripe_causal);
}
}

static ui64 rng_state = 0x243F6A8885A308D3ull;
static ui64 rnd() {
  rng_state ^= rng_state >> 12;
  rng_state ^= rng_state << 25;
  rng_state ^= rng_state >> 27;
  return rng_state * 0x2545F4914F6CDD1Dull;
}
static ui32 below(ui32 n) { return n ? (ui32)(rnd() % n) : 0; }

struct Case {
  int bits;
  bool causal;
  ui32 mmsb, passes, l1, l2, w, h, stride;
  std::vector<ui8> coded;
};

struct Seed {
  int bits;
  ui32 mmsb, l1, w, h;
  std::vector<ui8> coded;
};

static ui32 rd32(const ui8* p) {
  return p[0] | (p[1] << 8) | (p[2] << 16) | ((ui32)p[3] << 24);
}

static void put32(std::vector<ui8>& b, ui32 v) {
  for (int i = 0; i < 4; ++i) b.push_back((ui8)(v >> (8 * i)));
}

static ui64 fnv(const ui8* p, size_t n) {
  ui64 h = 0xcbf29ce484222325ull;
  for (size_t i = 0; i < n; ++i) { h ^= p[i]; h *= 0x100000001b3ull; }
  return h;
}

static bool list_mode = false;
static int case_no = 0;

// Runs one case at one alignment; returns hash of the buffer, sets result.
static ui64 run_at(const Case& c, int align, bool& result) {
  std::vector<ui8> mem(64 + c.coded.size() + 64);
  static ui64 g = 0x9E3779B97F4A7C15ull;     // separate generator, so the
  for (auto& b : mem) {                      // case stream is the same in
    g = g * 6364136223846793005ull + 1;      // list and write modes
    b = (ui8)(g >> 56);                      // (garbage around the data)
  }
  ui8* base = mem.data() + 16;
  base += (16 - ((uintptr_t)base & 15)) + align; // known alignment
  memcpy(base, c.coded.data(), c.coded.size());
  size_t n = (size_t)c.stride * (c.h + 4);
  if (c.bits == 32) {
    std::vector<ui32> out(n, 0);
    result = ojph::local::ojph_decode_codeblock32(base, out.data(), c.mmsb,
      c.passes, c.l1, c.l2, c.w, c.h, c.stride, c.causal);
    return fnv((const ui8*)out.data(), n * 4);
  } else {
    std::vector<ui64> out(n, 0);
    result = ojph::local::ojph_decode_codeblock64(base, out.data(), c.mmsb,
      c.passes, c.l1, c.l2, c.w, c.h, c.stride, c.causal);
    return fnv((const ui8*)out.data(), n * 8);
  }
}

// geometry the C++ scratch arrays can hold: use JPEG 2000 block limits
static bool geometry_ok(ui32 w, ui32 h) {
  if (w == 0 || h == 0 || w > 1024 || h > 1024) return false;
  ui32 W = 4, H = 4;
  while (W < w) W <<= 1;
  while (H < h) H <<= 1;
  return W * H <= 4096;
}

static std::vector<ui8> out_buf;
static std::set<int> skip;
static int written = 0, dropped_align = 0, skipped = 0;
static int stats_passes[4][2] = {};  // [passes][bits==64]
static int stats_false = 0, stats_causal = 0;

static void emit(Case c) {
  if (!geometry_ok(c.w, c.h)) return;
  if (c.l1 + c.l2 != c.coded.size()) return;
  c.stride = c.w + 8 + below(4);
  int no = case_no++;
  if (list_mode) {
    fprintf(stderr, "CASE %d\n", no);
    bool r;
    run_at(c, 0, r);
    return;
  }
  if (skip.count(no)) { ++skipped; return; }
  bool r0 = false;
  ui64 h0 = run_at(c, 0, r0);
  for (int a = 1; a < 4; ++a) {
    bool r;
    ui64 h = run_at(c, a, r);
    if (h != h0 || r != r0) {
      fprintf(stderr, "case %d: result differs between alignments\n", no);
      ++dropped_align;
      return;
    }
  }
  out_buf.push_back('O'); out_buf.push_back('J');
  out_buf.push_back('B'); out_buf.push_back('H');
  out_buf.push_back((ui8)c.bits);
  out_buf.push_back(c.causal ? 1 : 0);
  out_buf.push_back(r0 ? 1 : 0);
  out_buf.push_back(0);
  put32(out_buf, c.mmsb); put32(out_buf, c.passes);
  put32(out_buf, c.l1); put32(out_buf, c.l2);
  put32(out_buf, c.w); put32(out_buf, c.h); put32(out_buf, c.stride);
  put32(out_buf, (ui32)c.coded.size());
  out_buf.insert(out_buf.end(), c.coded.begin(), c.coded.end());
  for (int i = 0; i < 8; ++i) out_buf.push_back((ui8)(h0 >> (8 * i)));
  ++written;
  stats_passes[c.passes > 3 ? 0 : c.passes][c.bits == 64]++;
  stats_false += !r0;
  stats_causal += c.causal;
}

static std::vector<ui8> random_bytes(size_t n) {
  std::vector<ui8> v(n);
  for (auto& b : v) b = (ui8)rnd();
  return v;
}

static Case from_seed(const Seed& s) {
  Case c;
  c.bits = s.bits; c.causal = false; c.mmsb = s.mmsb; c.passes = 1;
  c.l1 = s.l1; c.l2 = 0; c.w = s.w; c.h = s.h; c.stride = 0;
  c.coded = s.coded;
  return c;
}

static void add_refinement(Case& c, ui32 passes) {
  c.passes = passes;
  c.l2 = 1 + below(below(4) == 0 ? 200 : 40);
  std::vector<ui8> r = random_bytes(c.l2);
  // sometimes bias towards stuffing-relevant values
  if (below(3) == 0)
    for (auto& b : r) if (below(4) == 0) b = below(2) ? 0xFF : 0x7F;
  c.coded.resize(c.l1);
  c.coded.insert(c.coded.end(), r.begin(), r.end());
  c.causal = below(2) != 0;
}

int main(int argc, char** argv) {
  if (argc < 4) { fprintf(stderr, "usage: see source\n"); return 2; }
  std::string mode = argv[1];
  if (mode == "list") list_mode = true;
  else if (mode.compare(0, 5, "write") == 0) {
    if (mode.size() > 6) {
      const char* p = mode.c_str() + 6;
      while (*p) { skip.insert(atoi(p)); while (*p && *p != ',') ++p; if (*p) ++p; }
    }
  } else { fprintf(stderr, "bad mode\n"); return 2; }

  std::vector<Seed> seeds;
  for (int i = 3; i < argc; ++i) {
    FILE* f = fopen(argv[i], "rb");
    if (!f) { perror(argv[i]); return 1; }
    std::vector<ui8> d;
    ui8 buf[65536]; size_t n;
    while ((n = fread(buf, 1, sizeof buf, f)) > 0) d.insert(d.end(), buf, buf + n);
    fclose(f);
    size_t off = 0;
    while (off + 40 <= d.size()) {
      const ui8* p = d.data() + off;
      Seed s;
      s.bits = p[4];
      s.mmsb = rd32(p + 8);
      s.l1 = rd32(p + 16);
      s.w = rd32(p + 24); s.h = rd32(p + 28);
      ui32 clen = rd32(p + 36);
      s.coded.assign(p + 40, p + 40 + clen);
      off += 40 + clen + (size_t)s.w * s.h * (s.bits / 8);
      if (clen <= 160 && rd32(p + 12) == 1) seeds.push_back(s);
    }
  }
  // subsample seeds deterministically: keep about 260 of them
  std::vector<Seed> picked;
  for (size_t i = 0; i < seeds.size(); ++i)
    if (below((ui32)seeds.size()) < 260) picked.push_back(seeds[i]);
  fprintf(stderr, "%zu seeds, %zu picked\n", seeds.size(), picked.size());

  for (const Seed& s : picked) {
    int b64 = s.bits == 64;
    // 1. refinement passes on the real cleanup pass
    for (ui32 passes = 2; passes <= 3; ++passes) {
      Case c = from_seed(s);
      if (!b64 && c.mmsb > 28) c.mmsb = 28;
      add_refinement(c, passes);
      emit(c);
    }
    // 2. bit flips in the cleanup pass, any pass count
    {
      Case c = from_seed(s);
      int nf = 1 + below(3);
      for (int i = 0; i < nf; ++i) c.coded[below(c.l1)] ^= (ui8)(1u << below(8));
      ui32 passes = 1 + below(3);
      if (passes > 1) { if (!b64 && c.mmsb > 28) c.mmsb = 28; add_refinement(c, passes); }
      emit(c);
    }
    // 3. truncated cleanup pass (scup is then read from the new end)
    {
      Case c = from_seed(s);
      if (c.l1 > 2) {
        c.l1 = 2 + below(c.l1 - 2);
        c.coded.resize(c.l1);
        emit(c);
      }
    }
    // 4. other missing_msbs (cleanup only, so the 64-bit 32-bit-shift
    //    in SigProp is not involved; sanitizer filters what remains)
    {
      Case c = from_seed(s);
      c.mmsb = b64 ? 26 + below(36) : below(32);
      emit(c);
    }
    // 5. smaller geometry, same data
    {
      Case c = from_seed(s);
      c.w = 1 + below(c.w);
      c.h = 1 + below(c.h);
      if (below(2)) { if (!b64 && c.mmsb > 28) c.mmsb = 28; add_refinement(c, 2 + below(2)); }
      emit(c);
    }
  }
  // 6. pure noise with a valid-looking scup, both decoders, all passes
  for (int i = 0; i < 400; ++i) {
    Case c;
    c.bits = below(3) == 0 ? 64 : 32;
    c.w = 1 + below(below(2) ? 8 : 40);
    c.h = 1 + below(below(2) ? 8 : 40);
    c.l1 = 2 + below(below(2) ? 12 : 70);
    c.coded = random_bytes(c.l1);
    ui32 scup = 2 + below(c.l1 - 1);
    c.coded[c.l1 - 1] = (ui8)(scup >> 4);
    c.coded[c.l1 - 2] = (ui8)((c.coded[c.l1 - 2] & 0xF0) | (scup & 0xF));
    c.mmsb = c.bits == 64 ? 28 + below(6) : 18 + below(11);
    c.l2 = 0; c.passes = 1; c.causal = false; c.stride = 0;
    ui32 passes = 1 + below(3);
    if (passes > 1) add_refinement(c, passes);
    emit(c);
  }

  // 7. 64-bit noise with large missing_msbs and many zero bytes, so that
  //    u_q > 32 quads (u_q extensions) occur and still decode
  for (int i = 0; i < 600; ++i) {
    Case c;
    c.bits = 64;
    c.w = 1 + below(below(2) ? 8 : 32);
    c.h = 1 + below(below(2) ? 8 : 32);
    c.l1 = 2 + below(below(2) ? 16 : 90);
    c.coded = random_bytes(c.l1);
    ui32 zp = 1 + below(4);  // zero-byte bias, and for long u suffixes
    static const ui8 pat[4] = {0x00, 0xF0, 0x0F, 0x08};  // runs of 0s/1s
    for (auto& b : c.coded)
      if (below(4) < zp) b = (i & 1) ? 0 : pat[below(4)];
    ui32 scup = 2 + below(c.l1 - 1);
    c.coded[c.l1 - 1] = (ui8)(scup >> 4);
    c.coded[c.l1 - 2] = (ui8)((c.coded[c.l1 - 2] & 0xF0) | (scup & 0xF));
    c.mmsb = 34 + below(28);
    c.l2 = 0; c.passes = 1; c.causal = false; c.stride = 0;
    if (below(4) == 0) add_refinement(c, 2 + below(2));
    emit(c);
  }

  if (list_mode) return 0;
  FILE* f = fopen(argv[2], "wb");
  if (!f) { perror(argv[2]); return 1; }
  fwrite(out_buf.data(), 1, out_buf.size(), f);
  fclose(f);
  fprintf(stderr, "cases %d, written %d (%zu bytes), skipped (UB) %d, "
          "dropped (alignment) %d, false %d, causal %d\n",
          case_no, written, out_buf.size(), skipped, dropped_align,
          stats_false, stats_causal);
  for (int p = 1; p <= 3; ++p)
    fprintf(stderr, "passes %d: 32-bit %d, 64-bit %d\n", p,
            stats_passes[p][0], stats_passes[p][1]);
  return 0;
}
