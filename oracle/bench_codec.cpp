// bench_codec: in-process, single-threaded, memory-to-memory timing
// harness for OpenJPH, the counterpart of the crate's
// examples/bench_codec.rs.
//
//   bench_codec check    [ojph_enc options]   encode once, decode once
//   bench_codec enc N    [ojph_enc options]   N encodes
//   bench_codec dec N    [ojph_enc options]   N decodes
//   bench_codec dump     [ojph_enc options]   the decode, .raw layout,
//                                             to stdout
//
// The options are ojph_enc's (enc_setup.h), without -o: the image is the
// one ojph_enc synthesises and the encoder gets the same calls.  The
// image is synthesised once, before the loop, and converted to the
// 32-bit samples that exchange() takes.  For `dec` the codestream is
// encoded once, also before the loop.
//
// Each encode builds a codestream object, writes to a fresh mem_outfile
// and pushes every line with exchange(); each decode builds a codestream
// object, reads from a mem_infile and pulls every line into a buffer
// allocated before the loop.  Nothing is written to disk.
//
// The output is one line of key=value pairs: the size and FNV-1a-64 of
// the codestream (check, enc) and of the decoded samples in ojph_dec's
// .raw layout (check, dec), from the last iteration.  The timing itself
// is left to perf stat; run N and 2N iterations and take the difference
// to remove the set-up.

#include "enc_setup.h"

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <string>
#include <vector>

using namespace enc_setup;

namespace {

struct Fnv {
  uint64_t h = 0xcbf29ce484222325ull;
  void byte(uint8_t b) { h = (h ^ b) * 0x100000001b3ull; }
  void bytes(const uint8_t* p, size_t n) { for (size_t i = 0; i < n; ++i) byte(p[i]); }
  void u32(uint32_t v) { for (int i = 0; i < 4; ++i) byte((uint8_t)(v >> (8 * i))); }
};

// The image as exchange() takes it: the low 32 bits of every sample.
typedef std::vector<std::vector<int32_t>> Image32;

Image32 to_i32(const std::vector<std::vector<int64_t>>& img) {
  Image32 out(img.size());
  for (size_t c = 0; c < img.size(); ++c) {
    out[c].resize(img[c].size());
    for (size_t i = 0; i < img[c].size(); ++i)
      out[c][i] = (int32_t)(uint32_t)(uint64_t)img[c][i];
  }
  return out;
}

// One encode into out, which must be closed (or new).
void encode(const EncArgs& e, const Image32& img, ojph::mem_outfile& out) {
  ojph::codestream cs;
  std::vector<std::vector<uint8_t>> luts;
  configure(cs, e, luts);
  out.open();
  std::vector<ojph::comment_exchange> coms = comments(e);
  cs.write_headers(&out, coms.empty() ? NULL : coms.data(),
                   (ojph::ui32)coms.size());

  const uint32_t nc = e.num_comps;
  uint64_t total_lines = 0;
  for (uint32_t c = 0; c < nc; ++c) total_lines += e.ch[c];
  uint32_t row[16384];
  if (nc > sizeof(row) / sizeof(row[0])) die("too many components");
  memset(row, 0, nc * sizeof(row[0]));
  ojph::ui32 next = 0;
  ojph::line_buf* line = cs.exchange(NULL, next);
  for (uint64_t n = 0; n < total_lines; ++n) {
    if (next >= nc || row[next] >= e.ch[next])
      die("exchange asked for an unexpected component/line");
    const uint32_t w = e.cw[next];
    const int32_t* src = &img[next][(size_t)row[next] * w];
    if (line->flags & ojph::line_buf::LFT_64BIT) {
      if (e.fmt[next].second)
        for (uint32_t x = 0; x < w; ++x) line->i64[x] = src[x];
      else
        for (uint32_t x = 0; x < w; ++x) line->i64[x] = (uint32_t)src[x];
    } else if (line->flags & ojph::line_buf::LFT_32BIT) {
      memcpy(line->i32, src, w * sizeof(int32_t));
    } else {
      die("unexpected line_buf flags");
    }
    row[next]++;
    line = cs.exchange(line, next);
  }
  cs.flush();
  // cs.close() would close (free) out; the caller does that.
}

// Decoded planes in the .raw layout's order, as 32-bit words.
struct Planes {
  std::vector<uint32_t> w, h;
  std::vector<std::vector<uint32_t>> data;
};

// One decode of data[0..size) into p, whose buffers are reused.
void decode(const uint8_t* data, size_t size, Planes& p) {
  ojph::mem_infile in;
  in.open(data, size);
  ojph::codestream cs;
  cs.read_headers(&in);
  ojph::param_siz siz = cs.access_siz();
  const uint32_t nc = siz.get_num_components();
  if (p.data.size() != nc) {
    p.w.assign(nc, 0);
    p.h.assign(nc, 0);
    p.data.assign(nc, std::vector<uint32_t>());
  }
  uint64_t total = 0;
  bool same_heights = true;
  for (uint32_t c = 0; c < nc; ++c) {
    p.w[c] = siz.get_recon_width(c);
    p.h[c] = siz.get_recon_height(c);
    total += p.h[c];
    if (p.data[c].size() != (size_t)p.w[c] * p.h[c])
      p.data[c].resize((size_t)p.w[c] * p.h[c]);
    if (p.h[c] != p.h[0]) same_heights = false;
  }
  // As ojph_dec: interleaved for the colour transform, else planar.
  cs.set_planar(!(same_heights && cs.access_cod().is_using_color_transform()));
  cs.create();
  uint32_t row[16384];
  if (nc > sizeof(row) / sizeof(row[0])) die("too many components");
  memset(row, 0, nc * sizeof(row[0]));
  for (uint64_t n = 0; n < total; ++n) {
    ojph::ui32 c = 0;
    ojph::line_buf* line = cs.pull(c);
    if (line == NULL || c >= nc || row[c] >= p.h[c])
      die("pull returned an unexpected line");
    const uint32_t w = p.w[c];
    uint32_t* dst = &p.data[c][(size_t)row[c] * w];
    if (line->flags & ojph::line_buf::LFT_64BIT) {
      for (uint32_t x = 0; x < w; ++x) dst[x] = (uint32_t)(uint64_t)line->i64[x];
    } else if ((line->flags & ojph::line_buf::LFT_32BIT) &&
               (line->flags & ojph::line_buf::LFT_INTEGER)) {
      memcpy(dst, line->i32, w * sizeof(uint32_t));
    } else {
      die("pull returned a float or unexpected line");
    }
    row[c]++;
  }
  cs.close();
}

void print_j2c(const ojph::mem_outfile& out) {
  Fnv f;
  f.bytes(out.get_data(), (size_t)out.get_used_size());
  printf("j2c_bytes=%zu j2c_fnv=%016llx", (size_t)out.get_used_size(),
         (unsigned long long)f.h);
}

void print_raw(const Planes& p) {
  Fnv f;
  size_t n = 0;
  for (const auto& d : p.data) {
    for (uint32_t v : d) f.u32(v);
    n += d.size() * 4;
  }
  printf("raw_bytes=%zu raw_fnv=%016llx", n, (unsigned long long)f.h);
}

const char* const USAGE =
  "usage: bench_codec check|dump|enc N|dec N [ojph_enc options, without -o]";

}  // namespace

int main(int argc, char** argv) {
  tool_name() = "bench_codec";
  if (argc < 2) { usage(USAGE); return 2; }
  const std::string mode = argv[1];
  long iters = 1;
  int first = 2;
  if (mode == "enc" || mode == "dec") {
    if (argc < 3) die("%s needs an iteration count", mode.c_str());
    iters = strtol(argv[2], nullptr, 10);
    if (iters < 1) die("bad iteration count %s", argv[2]);
    first = 3;
  } else if (mode != "check" && mode != "dump") {
    usage(USAGE);
    return 2;
  }
  EncArgs e;
  for (int i = first; i < argc; ++i)
    if (!parse_option(argc, argv, i, e)) { usage(USAGE); return 0; }
  if (e.out_path || e.raw_path) die("bench_codec writes no files");
  finish(e);

  try {
    const Image32 img = to_i32(synthesise(e));
    ojph::mem_outfile out;
    if (mode == "enc") {
      for (long k = 0; k < iters; ++k) {
        if (k) out.close();
        encode(e, img, out);
      }
      print_j2c(out);
    } else {
      encode(e, img, out);
      std::vector<uint8_t> j2c(out.get_data(), out.get_data() + out.get_used_size());
      if (mode == "check") { print_j2c(out); printf(" "); }
      out.close();
      Planes p;
      for (long k = 0; k < iters; ++k) decode(j2c.data(), j2c.size(), p);
      if (mode == "dump") {
        for (const auto& d : p.data)
          for (uint32_t v : d) {
            const uint8_t b[4] = {(uint8_t)v, (uint8_t)(v >> 8),
                                  (uint8_t)(v >> 16), (uint8_t)(v >> 24)};
            fwrite(b, 1, 4, stdout);
          }
        return 0;
      }
      print_raw(p);
    }
    printf("\n");
  } catch (const std::exception& ex) {
    fprintf(stderr, "bench_codec: OpenJPH exception: %s\n", ex.what());
    return 1;
  }
  return 0;
}
