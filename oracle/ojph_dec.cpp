// ojph_dec: reference decoder dump used as a test oracle.
//
// Decodes a raw J2K/HTJ2K codestream with OpenJPH and writes exactly what
// codestream::pull() returns, component by component (component 0 all lines
// top to bottom, then component 1, ...).  Every sample is written as 4 bytes
// little-endian holding the low 32 bits of the value returned by pull()
// (two's complement for signed components, plain u32 for unsigned ones).
// Component sizes are the reconstructed sizes from SIZ.
//
// usage: ojph_dec in.j2c out.raw [--info]
//   --info  print one line per component (size, depth, signedness) and the
//           COD summary to stdout

#include <openjph/ojph_arch.h>
#include <openjph/ojph_codestream.h>
#include <openjph/ojph_file.h>
#include <openjph/ojph_mem.h>
#include <openjph/ojph_params.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <vector>

int main(int argc, char** argv) {
  if (argc < 3) {
    fprintf(stderr, "usage: ojph_dec in.j2c out.raw [--info]\n");
    return 2;
  }
  const char* in_path = argv[1];
  const char* out_path = argv[2];
  bool info = argc > 3 && strcmp(argv[3], "--info") == 0;

  std::vector<std::vector<uint32_t>> planes;
  try {
    ojph::j2c_infile in;
    in.open(in_path);
    ojph::codestream cs;
    cs.read_headers(&in);
    ojph::param_siz siz = cs.access_siz();
    ojph::param_cod cod = cs.access_cod();
    const uint32_t nc = siz.get_num_components();
    std::vector<uint32_t> w(nc), h(nc), row(nc, 0);
    uint64_t total = 0;
    for (uint32_t c = 0; c < nc; ++c) {
      w[c] = siz.get_recon_width(c);
      h[c] = siz.get_recon_height(c);
      total += h[c];
      planes.emplace_back((size_t)w[c] * h[c], 0u);
    }
    if (info) {
      for (uint32_t c = 0; c < nc; ++c) {
        ojph::point d = siz.get_downsampling(c);
        ojph::ui8 nbd = 0, nty = 0;
        bool nsg = false;
        int nlt = cs.access_nlt().get_nonlinear_transform(c, nbd, nsg, nty)
                    ? (int)nty : 0;
        printf("comp %u: %ux%u depth %u %s sub %ux%u nlt %d\n", c, w[c], h[c],
               siz.get_bit_depth(c), siz.is_signed(c) ? "signed" : "unsigned",
               d.x, d.y, nlt);
      }
      printf("cod: %s levels %u block %ux%u prog %s rct %d\n",
             cod.is_reversible() ? "reversible" : "irreversible",
             cod.get_num_decompositions(), cod.get_block_dims().w,
             cod.get_block_dims().h, cod.get_progression_order_as_string(),
             (int)cod.is_using_color_transform());
    }
    // Planar mode is required when components are subsampled; it is also
    // valid otherwise, and the per-component row counters below make the
    // output independent of the order in which pull() returns lines.
    bool subsampled = false;
    for (uint32_t c = 0; c < nc; ++c) {
      ojph::point d = siz.get_downsampling(c);
      if (d.x != 1 || d.y != 1) subsampled = true;
    }
    cs.set_planar(subsampled || cod.is_using_color_transform() == false);
    cs.create();
    for (uint64_t n = 0; n < total; ++n) {
      ojph::ui32 c = 0;
      ojph::line_buf* line = cs.pull(c);
      if (line == NULL || c >= nc || row[c] >= h[c]) {
        fprintf(stderr, "ojph_dec: pull returned an unexpected line\n");
        return 1;
      }
      uint32_t* dst = &planes[c][(size_t)row[c] * w[c]];
      if (line->flags & ojph::line_buf::LFT_64BIT) {
        for (uint32_t x = 0; x < w[c]; ++x) dst[x] = (uint32_t)(uint64_t)line->i64[x];
      } else if (line->flags & ojph::line_buf::LFT_32BIT) {
        if (!(line->flags & ojph::line_buf::LFT_INTEGER)) {
          fprintf(stderr, "ojph_dec: pull returned a float line\n");
          return 1;
        }
        for (uint32_t x = 0; x < w[c]; ++x) dst[x] = (uint32_t)line->i32[x];
      } else {
        fprintf(stderr, "ojph_dec: unexpected line_buf flags 0x%x\n", line->flags);
        return 1;
      }
      row[c]++;
    }
    cs.close();
  } catch (const std::exception& e) {
    fprintf(stderr, "ojph_dec: OpenJPH exception: %s\n", e.what());
    return 1;
  }

  FILE* f = fopen(out_path, "wb");
  if (!f) { fprintf(stderr, "ojph_dec: cannot open %s\n", out_path); return 1; }
  std::vector<uint8_t> buf;
  for (auto& p : planes) {
    buf.resize(p.size() * 4);
    for (size_t i = 0; i < p.size(); ++i) {
      uint32_t u = p[i];
      buf[4 * i] = (uint8_t)u; buf[4 * i + 1] = (uint8_t)(u >> 8);
      buf[4 * i + 2] = (uint8_t)(u >> 16); buf[4 * i + 3] = (uint8_t)(u >> 24);
    }
    if (!buf.empty() && fwrite(buf.data(), 1, buf.size(), f) != buf.size()) {
      fprintf(stderr, "ojph_dec: write failed\n");
      fclose(f);
      return 1;
    }
  }
  fclose(f);
  return 0;
}
