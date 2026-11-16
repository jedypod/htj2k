// ojph_enc: parametrised OpenJPH encoder used as a test oracle.
//
// Synthesises a deterministic test image and encodes it with OpenJPH into a
// raw J2K/HTJ2K codestream (.j2c, no JP2/JPH boxes).  Optionally also writes
// the samples it fed to the encoder in the same .raw layout that ojph_dec
// produces, so a lossless round trip can be checked with cmp(1).
//
// Run with --help for the option list.

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
#include <string>
#include <vector>

namespace {

// ---------------------------------------------------------------- PRNG
// splitmix64 (Steele, Lea, Flood 2014).  Fully specified here so the data is
// reproducible on any platform.
struct SplitMix64 {
  uint64_t s;
  explicit SplitMix64(uint64_t seed) : s(seed) {}
  uint64_t next() {
    uint64_t z = (s += 0x9E3779B97F4A7C15ull);
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
    return z ^ (z >> 31);
  }
};

[[noreturn]] void die(const char* fmt, const char* a = "") {
  fprintf(stderr, "ojph_enc: ");
  fprintf(stderr, fmt, a);
  fprintf(stderr, "\n");
  exit(2);
}

void usage() {
  printf(
"usage: ojph_enc -o out.j2c [options]\n"
"  -o FILE            output codestream (.j2c)\n"
"  --raw-out FILE     also write the input samples (ojph_dec .raw layout)\n"
"  --size WxH         image (region) size, default 64x64\n"
"  --comps N          number of components, default 1\n"
"  --depth B          bit depth 1..32 for every component, default 8\n"
"  --signed 0|1       signedness for every component, default 0\n"
"  --irrev            irreversible 9/7 (default reversible 5/3)\n"
"  --qstep F          irreversible base quantisation step (default 1/256)\n"
"  --rct 0|1          colour transform (needs >= 3 comps), default 0\n"
"  --levels N         decompositions, default 5\n"
"  --block WxH        code-block size, default 64x64\n"
"  --precincts LIST   comma list of WxH, index 0 = lowest resolution;\n"
"                     the last entry repeats for higher resolutions\n"
"  --coc C:LEVELS:WxH per-component override (COC); repeatable\n"
"  --prog ORDER       LRCP|RLCP|RPCL|PCRL|CPRL, default LRCP\n"
"  --tile WxH         tile size (default: one tile covering the image)\n"
"  --tile-off X,Y     tile grid offset (XTOsiz,YTOsiz), default 0,0\n"
"  --img-off X,Y      image offset (XOsiz,YOsiz), default 0,0\n"
"  --tlm              write a TLM marker\n"
"  --tileparts R|C|RC tile-part divisions at resolutions and/or components\n"
"  --nlt3             NLT type 3 on every signed component\n"
"  --nlt3-comps LIST  NLT type 3 on the listed (signed) components\n"
"  --sub LIST         comma list of DXxDY per component (missing = 1x1)\n"
"  --planar 0|1       planar exchange order (default: 0 with --rct 1,\n"
"                     else 1 if subsampled or comps==1, else 0)\n"
"  --seed N           PRNG seed, default 1\n"
"  --pattern P        smooth|noise|const|zero, default smooth\n");
}

bool parse_wh(const char* s, uint32_t& w, uint32_t& h) {
  return sscanf(s, "%ux%u", &w, &h) == 2;
}
bool parse_xy(const char* s, uint32_t& x, uint32_t& y) {
  return sscanf(s, "%u,%u", &x, &y) == 2;
}
std::vector<std::string> split(const std::string& s, char sep) {
  std::vector<std::string> out;
  size_t b = 0;
  while (true) {
    size_t e = s.find(sep, b);
    out.push_back(s.substr(b, e == std::string::npos ? std::string::npos : e - b));
    if (e == std::string::npos) break;
    b = e + 1;
  }
  return out;
}

uint32_t div_ceil(uint32_t a, uint32_t b) { return (uint32_t)(((uint64_t)a + b - 1) / b); }

struct Coc { uint32_t comp, levels, bw, bh; };

}  // namespace

int main(int argc, char** argv) {
  const char* out_path = nullptr;
  const char* raw_path = nullptr;
  uint32_t width = 64, height = 64, num_comps = 1, depth = 8;
  bool is_signed = false, reversible = true, rct = false;
  float qstep = 1.0f / 256.0f;
  uint32_t levels = 5, bw = 64, bh = 64;
  std::vector<ojph::size> precincts;
  std::vector<Coc> cocs;
  std::string prog = "LRCP";
  bool have_tile = false;
  uint32_t tw = 0, th = 0, tox = 0, toy = 0, iox = 0, ioy = 0;
  bool tlm = false, tp_res = false, tp_comp = false;
  bool nlt_all = false;
  std::vector<uint32_t> nlt_comps;
  std::vector<std::pair<uint32_t, uint32_t>> sub;
  int planar = -1;
  uint64_t seed = 1;
  std::string pattern = "smooth";

  for (int i = 1; i < argc; ++i) {
    std::string a = argv[i];
    auto val = [&]() -> const char* {
      if (i + 1 >= argc) die("missing value for %s", a.c_str());
      return argv[++i];
    };
    if (a == "-h" || a == "--help") { usage(); return 0; }
    else if (a == "-o") out_path = val();
    else if (a == "--raw-out") raw_path = val();
    else if (a == "--size") { if (!parse_wh(val(), width, height)) die("bad --size"); }
    else if (a == "--comps") num_comps = (uint32_t)atoi(val());
    else if (a == "--depth") depth = (uint32_t)atoi(val());
    else if (a == "--signed") is_signed = atoi(val()) != 0;
    else if (a == "--irrev") reversible = false;
    else if (a == "--qstep") qstep = (float)atof(val());
    else if (a == "--rct") rct = atoi(val()) != 0;
    else if (a == "--levels") levels = (uint32_t)atoi(val());
    else if (a == "--block") { if (!parse_wh(val(), bw, bh)) die("bad --block"); }
    else if (a == "--precincts") {
      for (auto& p : split(val(), ',')) {
        uint32_t w, h;
        if (!parse_wh(p.c_str(), w, h)) die("bad --precincts");
        precincts.push_back(ojph::size(w, h));
      }
    }
    else if (a == "--coc") {
      Coc c;
      if (sscanf(val(), "%u:%u:%ux%u", &c.comp, &c.levels, &c.bw, &c.bh) != 4)
        die("bad --coc");
      cocs.push_back(c);
    }
    else if (a == "--prog") prog = val();
    else if (a == "--tile") { if (!parse_wh(val(), tw, th)) die("bad --tile"); have_tile = true; }
    else if (a == "--tile-off") { if (!parse_xy(val(), tox, toy)) die("bad --tile-off"); }
    else if (a == "--img-off") { if (!parse_xy(val(), iox, ioy)) die("bad --img-off"); }
    else if (a == "--tlm") tlm = true;
    else if (a == "--tileparts") {
      std::string t = val();
      tp_res = t.find('R') != std::string::npos;
      tp_comp = t.find('C') != std::string::npos;
    }
    else if (a == "--nlt3") nlt_all = true;
    else if (a == "--nlt3-comps") {
      for (auto& p : split(val(), ',')) nlt_comps.push_back((uint32_t)atoi(p.c_str()));
    }
    else if (a == "--sub") {
      for (auto& p : split(val(), ',')) {
        uint32_t dx, dy;
        if (!parse_wh(p.c_str(), dx, dy)) die("bad --sub");
        sub.push_back({dx, dy});
      }
    }
    else if (a == "--planar") planar = atoi(val());
    else if (a == "--seed") seed = strtoull(val(), nullptr, 0);
    else if (a == "--pattern") pattern = val();
    else die("unknown option %s", a.c_str());
  }
  if (!out_path) { usage(); return 2; }
  if (depth < 1 || depth > 32) die("depth must be 1..32");
  if (num_comps < 1) die("need at least one component");
  if (pattern != "smooth" && pattern != "noise" && pattern != "const" &&
      pattern != "zero")
    die("unknown pattern %s", pattern.c_str());
  while (sub.size() < num_comps) sub.push_back({1, 1});
  bool subsampled = false;
  for (uint32_t c = 0; c < num_comps; ++c)
    if (sub[c].first != 1 || sub[c].second != 1) subsampled = true;
  if (planar < 0) planar = rct ? 0 : ((subsampled || num_comps == 1) ? 1 : 0);

  // Component geometry (reference grid: Xsiz = XOsiz + width).
  const uint32_t Xsiz = iox + width, Ysiz = ioy + height;
  std::vector<uint32_t> cw(num_comps), ch(num_comps);
  for (uint32_t c = 0; c < num_comps; ++c) {
    cw[c] = div_ceil(Xsiz, sub[c].first) - div_ceil(iox, sub[c].first);
    ch[c] = div_ceil(Ysiz, sub[c].second) - div_ceil(ioy, sub[c].second);
  }

  // Legal sample range.
  int64_t lo, hi;
  if (is_signed) { lo = -((int64_t)1 << (depth - 1)); hi = ((int64_t)1 << (depth - 1)) - 1; }
  else { lo = 0; hi = ((int64_t)1 << depth) - 1; }
  const uint64_t span = (uint64_t)(hi - lo) + 1;  // <= 2^32

  // Synthesise the image: img[c][y*cw+x].
  std::vector<std::vector<int64_t>> img(num_comps);
  SplitMix64 rng(seed);
  for (uint32_t c = 0; c < num_comps; ++c) {
    img[c].resize((size_t)cw[c] * ch[c]);
    int64_t cval = lo + (int64_t)(rng.next() % span);
    // gradient direction / phase per component
    uint64_t ph = rng.next();
    double fx = 0.5 + (double)(ph & 0xFF) / 512.0;
    double fy = 0.5 + (double)((ph >> 8) & 0xFF) / 512.0;
    int64_t namp = (int64_t)(span >> 6);  // ~1.5% of the range
    if (namp < 1) namp = 1;
    for (uint32_t y = 0; y < ch[c]; ++y)
      for (uint32_t x = 0; x < cw[c]; ++x) {
        int64_t v = 0;
        if (pattern == "zero") v = 0;
        else if (pattern == "const") v = cval;
        else if (pattern == "noise") v = lo + (int64_t)(rng.next() % span);
        else {  // smooth
          double tx = cw[c] > 1 ? (double)x / (cw[c] - 1) : 0.0;
          double ty = ch[c] > 1 ? (double)y / (ch[c] - 1) : 0.0;
          double t = (fx * tx + fy * ty) / (fx + fy);  // 0..1
          int64_t base = lo + (int64_t)(t * (double)(span - 1));
          int64_t n = (int64_t)(rng.next() % (uint64_t)(2 * namp + 1)) - namp;
          v = base + n;
          if (v < lo) v = lo;
          if (v > hi) v = hi;
        }
        img[c][(size_t)y * cw[c] + x] = v;
      }
  }

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
    ojph::param_siz siz = cs.access_siz();
    siz.set_image_extent(ojph::point(Xsiz, Ysiz));
    siz.set_image_offset(ojph::point(iox, ioy));
    if (have_tile) siz.set_tile_size(ojph::size(tw, th));
    else siz.set_tile_size(ojph::size(Xsiz, Ysiz));
    siz.set_tile_offset(ojph::point(tox, toy));
    siz.set_num_components(num_comps);
    for (uint32_t c = 0; c < num_comps; ++c)
      siz.set_component(c, ojph::point(sub[c].first, sub[c].second), depth,
                        is_signed);

    ojph::param_cod cod = cs.access_cod();
    cod.set_num_decomposition(levels);
    cod.set_block_dims(bw, bh);
    if (!precincts.empty())
      cod.set_precinct_size((int)precincts.size(), precincts.data());
    cod.set_progression_order(prog.c_str());
    cod.set_color_transform(rct);
    cod.set_reversible(reversible);
    for (const Coc& c : cocs) {
      cod.set_num_decomposition(c.comp, c.levels);
      cod.set_block_dims(c.comp, c.bw, c.bh);
      cod.set_reversible(c.comp, reversible);
    }
    if (!reversible) cs.access_qcd().set_irrev_quant(qstep);

    ojph::param_nlt nlt = cs.access_nlt();
    for (uint32_t c = 0; c < num_comps; ++c) {
      bool want = false;
      if (nlt_all && is_signed) want = true;
      for (uint32_t n : nlt_comps) if (n == c) want = true;
      if (want)
        nlt.set_nonlinear_transform(
          c, ojph::param_nlt::nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT);
    }

    cs.set_planar(planar != 0);
    if (tlm) cs.request_tlm_marker(true);
    if (tp_res || tp_comp) cs.set_tilepart_divisions(tp_res, tp_comp);

    ojph::j2c_outfile out;
    out.open(out_path);
    cs.write_headers(&out);

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
