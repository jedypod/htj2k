// ojph_enc: parametrised OpenJPH encoder used as a test oracle.
//
// Synthesises a deterministic test image and encodes it with OpenJPH into a
// raw J2K/HTJ2K codestream (.j2c, no JP2/JPH boxes).  Optionally also writes
// the samples it fed to the encoder in the same .raw layout that ojph_dec
// produces, so a lossless round trip can be checked with cmp(1).
//
// Run with --help for the option list.

#include "enc_setup.h"

#include <cstdint>
#include <cstdio>
#include <exception>
#include <vector>

using namespace enc_setup;

int main(int argc, char** argv) {
  EncArgs e;
  for (int i = 1; i < argc; ++i)
    if (!parse_option(argc, argv, i, e)) {
      usage("usage: ojph_enc -o out.j2c [options]");
      return 0;
    }
  if (!e.out_path) { usage("usage: ojph_enc -o out.j2c [options]"); return 2; }
  finish(e);
  const char* out_path = e.out_path;
  const char* raw_path = e.raw_path;
  const uint32_t num_comps = e.num_comps;
  const std::vector<uint32_t>& cw = e.cw;
  const std::vector<uint32_t>& ch = e.ch;
  std::vector<std::vector<int64_t>> img = synthesise(e);

  if (raw_path) {
    FILE* f = fopen(raw_path, "wb");
    if (!f) die("cannot open %s", raw_path);
    for (uint32_t c = 0; c < num_comps; ++c)
      for (int64_t v : img[c]) {
        uint32_t u = (uint32_t)(uint64_t)v;
        uint8_t b[4] = {(uint8_t)u, (uint8_t)(u >> 8), (uint8_t)(u >> 16), (uint8_t)(u >> 24)};
        fwrite(b, 1, 4, f);
      }
    fclose(f);
  }

  try {
    ojph::codestream cs;
    std::vector<std::vector<uint8_t>> luts;
    configure(cs, e, luts);

    ojph::j2c_outfile out;
    out.open(out_path);
    std::vector<ojph::comment_exchange> coms = comments(e);
    cs.write_headers(&out, coms.empty() ? NULL : coms.data(),
                     (ojph::ui32)coms.size());

    uint64_t total_lines = 0;
    for (uint32_t c = 0; c < num_comps; ++c) total_lines += ch[c];
    std::vector<uint32_t> row(num_comps, 0);
    ojph::ui32 next = 0;
    ojph::line_buf* line = cs.exchange(NULL, next);
    for (uint64_t n = 0; n < total_lines; ++n) {
      if (next >= num_comps || row[next] >= ch[next])
        die("exchange asked for an unexpected component/line");
      const int64_t* src = &img[next][(size_t)row[next] * cw[next]];
      if (line->flags & ojph::line_buf::LFT_64BIT) {
        for (uint32_t x = 0; x < cw[next]; ++x) line->i64[x] = src[x];
      } else if (line->flags & ojph::line_buf::LFT_32BIT) {
        // Values above INT32_MAX (unsigned 32-bit) are passed as their bit
        // pattern, the usual way to hand such samples to OpenJPH.
        for (uint32_t x = 0; x < cw[next]; ++x)
          line->i32[x] = (ojph::si32)(uint32_t)(uint64_t)src[x];
      } else {
        die("unexpected line_buf flags");
      }
      row[next]++;
      line = cs.exchange(line, next);
    }
    cs.flush();
    cs.close();
  } catch (const std::exception& e) {
    fprintf(stderr, "ojph_enc: OpenJPH exception: %s\n", e.what());
    remove(out_path);
    return 1;
  }
  return 0;
}
