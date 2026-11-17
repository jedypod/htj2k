// enc_setup.h: the option parsing, image synthesis and encoder set-up of
// ojph_enc, shared with bench_codec so that both encode exactly the same
// image with exactly the same calls.
//
// Header-only; each tool is a single translation unit.

#ifndef ORACLE_ENC_SETUP_H
#define ORACLE_ENC_SETUP_H

#include <openjph/ojph_arch.h>
#include <openjph/ojph_codestream.h>
#include <openjph/ojph_file.h>
#include <openjph/ojph_mem.h>
#include <openjph/ojph_params.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <utility>
#include <vector>

namespace enc_setup {

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

// The prefix of die() messages, set by each tool.
inline const char*& tool_name() {
  static const char* name = "ojph_enc";
  return name;
}

[[noreturn]] inline void die(const char* fmt, const char* a = "") {
  fprintf(stderr, "%s: ", tool_name());
  fprintf(stderr, fmt, a);
  fprintf(stderr, "\n");
  exit(2);
}

inline void usage(const char* first_line) {
  printf("%s\n", first_line);
  printf(
"  -o FILE            output codestream (.j2c)\n"
"  --raw-out FILE     also write the input samples (ojph_dec .raw layout)\n"
"  --size WxH         image (region) size, default 64x64\n"
"  --comps N          number of components, default 1\n"
"  --depth B          bit depth 1..32 for every component, default 8\n"
"  --signed 0|1       signedness for every component, default 0\n"
"  --comp-fmt LIST    comma list of B:S (depth:signed) per component,\n"
"                     overriding --depth/--signed (missing = those)\n"
"  --irrev            irreversible 9/7 (default reversible 5/3)\n"
"  --qstep F          irreversible base quantisation step (default 1/256)\n"
"  --qstep-comp C:F   irreversible quantisation step of component C (QCC);\n"
"                     repeatable\n"
"  --qfactor Q        Qfactor 1..100 instead of the base step\n"
"  --rct 0|1          colour transform (needs >= 3 comps), default 0\n"
"  --levels N         decompositions, default 5\n"
"  --block WxH        code-block size, default 64x64\n"
"  --precincts LIST   comma list of WxH, index 0 = lowest resolution;\n"
"                     the last entry repeats for higher resolutions\n"
"  --coc C:LEVELS:WxH per-component override (COC); repeatable\n"
"  --rev-comps LIST   comma list of components made reversible (COC)\n"
"  --irrev-comps LIST comma list of components made irreversible (COC)\n"
"  --prog ORDER       LRCP|RLCP|RPCL|PCRL|CPRL, default LRCP\n"
"  --tile WxH         tile size (default: one tile covering the image)\n"
"  --tile-off X,Y     tile grid offset (XTOsiz,YTOsiz), default 0,0\n"
"  --img-off X,Y      image offset (XOsiz,YOsiz), default 0,0\n"
"  --tlm              write a TLM marker\n"
"  --tileparts R|C|RC tile-part divisions at resolutions and/or components\n"
"  --nlt3             NLT type 3 on every signed component\n"
"  --nlt3-comps LIST  NLT type 3 on the listed (signed) components\n"
"  --nlt4 C:N         NLT type 4 on component C, with an N-point LUT\n"
"                     (2..8192; formula in oracle/README.md); repeatable\n"
"  --comment TEXT     a COM segment with TEXT, after the version comment;\n"
"                     repeatable\n"
"  --sub LIST         comma list of DXxDY per component (missing = 1x1)\n"
"  --planar 0|1       planar exchange order (default: 0 with --rct 1,\n"
"                     else 1 if subsampled or comps==1, else 0)\n"
"  --seed N           PRNG seed, default 1\n"
"  --pattern P        smooth|noise|const|zero, default smooth\n");
}

inline bool parse_wh(const char* s, uint32_t& w, uint32_t& h) {
  return sscanf(s, "%ux%u", &w, &h) == 2;
}
inline bool parse_xy(const char* s, uint32_t& x, uint32_t& y) {
  return sscanf(s, "%u,%u", &x, &y) == 2;
}
inline std::vector<std::string> split(const std::string& s, char sep) {
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

inline uint32_t div_ceil(uint32_t a, uint32_t b) {
  return (uint32_t)(((uint64_t)a + b - 1) / b);
}

struct Coc { uint32_t comp, levels, bw, bh; };
struct Nlt4 { uint32_t comp, points; };

// The LUT of --nlt4: N points rising from 0 to M = 2^B - 1 for a
// component of depth B,
//   lut[i] = floor(M * i * (i + N - 1) / (2 * (N - 1)^2)),
// a strictly increasing quadratic (its slope at 0 is half the mean).
// The products stay below 2^63 for B <= 32 and N <= 8192.
inline std::vector<uint64_t> nlt4_lut(uint32_t depth, uint32_t n) {
  const uint64_t m = ((uint64_t)1 << depth) - 1, d = (uint64_t)(n - 1);
  std::vector<uint64_t> lut(n);
  for (uint64_t i = 0; i < n; ++i) lut[i] = m * i * (i + d) / (2 * d * d);
  return lut;
}

// Everything the options set, plus the derived geometry.
struct EncArgs {
  const char* out_path = nullptr;
  const char* raw_path = nullptr;
  uint32_t width = 64, height = 64, num_comps = 1, depth = 8;
  bool is_signed = false, reversible = true, rct = false;
  float qstep = 1.0f / 256.0f;
  bool have_qfactor = false;
  float qfactor = 0.0f;
  std::vector<std::pair<uint32_t, float>> qstep_comps;
  std::vector<uint32_t> rev_comps, irrev_comps;
  std::vector<Nlt4> nlt4s;
  std::vector<std::string> comments;
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
  std::vector<std::pair<uint32_t, bool>> fmt;
  int planar = -1;
  uint64_t seed = 1;
  std::string pattern = "smooth";

  // Derived by finish(): reference grid (Xsiz = XOsiz + width) and the
  // size of each component.
  uint32_t Xsiz = 0, Ysiz = 0;
  std::vector<uint32_t> cw, ch;
};

// Parses one option at argv[i] (advancing i past its value).  Returns 1 if
// it was an option, 0 for -h/--help, and dies on an unknown option.
inline int parse_option(int argc, char** argv, int& i, EncArgs& e) {
  std::string a = argv[i];
  auto val = [&]() -> const char* {
    if (i + 1 >= argc) die("missing value for %s", a.c_str());
    return argv[++i];
  };
  if (a == "-h" || a == "--help") return 0;
  else if (a == "-o") e.out_path = val();
  else if (a == "--raw-out") e.raw_path = val();
  else if (a == "--size") { if (!parse_wh(val(), e.width, e.height)) die("bad --size"); }
  else if (a == "--comps") e.num_comps = (uint32_t)atoi(val());
  else if (a == "--depth") e.depth = (uint32_t)atoi(val());
  else if (a == "--signed") e.is_signed = atoi(val()) != 0;
  else if (a == "--irrev") e.reversible = false;
  else if (a == "--qstep") e.qstep = (float)atof(val());
  else if (a == "--qstep-comp") {
    std::string v = val();
    size_t colon = v.find(':');
    if (colon == std::string::npos) die("bad --qstep-comp");
    e.qstep_comps.push_back({(uint32_t)atoi(v.substr(0, colon).c_str()),
                             (float)atof(v.c_str() + colon + 1)});
  }
  else if (a == "--qfactor") { e.have_qfactor = true; e.qfactor = (float)atof(val()); }
  else if (a == "--rev-comps") {
    for (auto& p : split(val(), ',')) e.rev_comps.push_back((uint32_t)atoi(p.c_str()));
  }
  else if (a == "--irrev-comps") {
    for (auto& p : split(val(), ',')) e.irrev_comps.push_back((uint32_t)atoi(p.c_str()));
  }
  else if (a == "--nlt4") {
    Nlt4 n;
    if (sscanf(val(), "%u:%u", &n.comp, &n.points) != 2) die("bad --nlt4");
    if (n.points < 2 || n.points > 8192) die("--nlt4 needs 2..8192 points");
    e.nlt4s.push_back(n);
  }
  else if (a == "--comment") e.comments.push_back(val());
  else if (a == "--rct") e.rct = atoi(val()) != 0;
  else if (a == "--levels") e.levels = (uint32_t)atoi(val());
  else if (a == "--block") { if (!parse_wh(val(), e.bw, e.bh)) die("bad --block"); }
  else if (a == "--precincts") {
    for (auto& p : split(val(), ',')) {
      uint32_t w, h;
      if (!parse_wh(p.c_str(), w, h)) die("bad --precincts");
      e.precincts.push_back(ojph::size(w, h));
    }
  }
  else if (a == "--coc") {
    Coc c;
    if (sscanf(val(), "%u:%u:%ux%u", &c.comp, &c.levels, &c.bw, &c.bh) != 4)
      die("bad --coc");
    e.cocs.push_back(c);
  }
  else if (a == "--prog") e.prog = val();
  else if (a == "--tile") { if (!parse_wh(val(), e.tw, e.th)) die("bad --tile"); e.have_tile = true; }
  else if (a == "--tile-off") { if (!parse_xy(val(), e.tox, e.toy)) die("bad --tile-off"); }
  else if (a == "--img-off") { if (!parse_xy(val(), e.iox, e.ioy)) die("bad --img-off"); }
  else if (a == "--tlm") e.tlm = true;
  else if (a == "--tileparts") {
    std::string t = val();
    e.tp_res = t.find('R') != std::string::npos;
    e.tp_comp = t.find('C') != std::string::npos;
  }
  else if (a == "--nlt3") e.nlt_all = true;
  else if (a == "--nlt3-comps") {
    for (auto& p : split(val(), ',')) e.nlt_comps.push_back((uint32_t)atoi(p.c_str()));
  }
  else if (a == "--sub") {
    for (auto& p : split(val(), ',')) {
      uint32_t dx, dy;
      if (!parse_wh(p.c_str(), dx, dy)) die("bad --sub");
      e.sub.push_back({dx, dy});
    }
  }
  else if (a == "--comp-fmt") {
    for (auto& p : split(val(), ',')) {
      uint32_t b, sg;
      if (sscanf(p.c_str(), "%u:%u", &b, &sg) != 2) die("bad --comp-fmt");
      e.fmt.push_back({b, sg != 0});
    }
  }
  else if (a == "--planar") e.planar = atoi(val());
  else if (a == "--seed") e.seed = strtoull(val(), nullptr, 0);
  else if (a == "--pattern") e.pattern = val();
  else die("unknown option %s", a.c_str());
  return 1;
}

// Validates the options and fills in the defaults and the geometry.
inline void finish(EncArgs& e) {
  if (e.depth < 1 || e.depth > 32) die("depth must be 1..32");
  if (e.num_comps < 1) die("need at least one component");
  if (e.pattern != "smooth" && e.pattern != "noise" && e.pattern != "const" &&
      e.pattern != "zero")
    die("unknown pattern %s", e.pattern.c_str());
  while (e.sub.size() < e.num_comps) e.sub.push_back({1, 1});
  while (e.fmt.size() < e.num_comps) e.fmt.push_back({e.depth, e.is_signed});
  for (uint32_t c = 0; c < e.num_comps; ++c)
    if (e.fmt[c].first < 1 || e.fmt[c].first > 32) die("depth must be 1..32");
  for (const Nlt4& n : e.nlt4s)
    if (n.comp >= e.num_comps) die("--nlt4 names a missing component");
  bool subsampled = false;
  for (uint32_t c = 0; c < e.num_comps; ++c)
    if (e.sub[c].first != 1 || e.sub[c].second != 1) subsampled = true;
  if (e.planar < 0) e.planar = e.rct ? 0 : ((subsampled || e.num_comps == 1) ? 1 : 0);

  e.Xsiz = e.iox + e.width;
  e.Ysiz = e.ioy + e.height;
  e.cw.assign(e.num_comps, 0);
  e.ch.assign(e.num_comps, 0);
  for (uint32_t c = 0; c < e.num_comps; ++c) {
    e.cw[c] = div_ceil(e.Xsiz, e.sub[c].first) - div_ceil(e.iox, e.sub[c].first);
    e.ch[c] = div_ceil(e.Ysiz, e.sub[c].second) - div_ceil(e.ioy, e.sub[c].second);
  }
}

// Synthesises the image: img[c][y*cw+x].
inline std::vector<std::vector<int64_t>> synthesise(const EncArgs& e) {
  std::vector<std::vector<int64_t>> img(e.num_comps);
  SplitMix64 rng(e.seed);
  for (uint32_t c = 0; c < e.num_comps; ++c) {
    const uint32_t cw = e.cw[c], ch = e.ch[c];
    // Legal sample range of this component.
    const uint32_t cdepth = e.fmt[c].first;
    int64_t lo, hi;
    if (e.fmt[c].second) { lo = -((int64_t)1 << (cdepth - 1)); hi = ((int64_t)1 << (cdepth - 1)) - 1; }
    else { lo = 0; hi = ((int64_t)1 << cdepth) - 1; }
    const uint64_t span = (uint64_t)(hi - lo) + 1;  // <= 2^32
    img[c].resize((size_t)cw * ch);
    int64_t cval = lo + (int64_t)(rng.next() % span);
    // gradient direction / phase per component
    uint64_t ph = rng.next();
    double fx = 0.5 + (double)(ph & 0xFF) / 512.0;
    double fy = 0.5 + (double)((ph >> 8) & 0xFF) / 512.0;
    int64_t namp = (int64_t)(span >> 6);  // ~1.5% of the range
    if (namp < 1) namp = 1;
    for (uint32_t y = 0; y < ch; ++y)
      for (uint32_t x = 0; x < cw; ++x) {
        int64_t v = 0;
        if (e.pattern == "zero") v = 0;
        else if (e.pattern == "const") v = cval;
        else if (e.pattern == "noise") v = lo + (int64_t)(rng.next() % span);
        else {  // smooth
          double tx = cw > 1 ? (double)x / (cw - 1) : 0.0;
          double ty = ch > 1 ? (double)y / (ch - 1) : 0.0;
          double t = (fx * tx + fy * ty) / (fx + fy);  // 0..1
          int64_t base = lo + (int64_t)(t * (double)(span - 1));
          int64_t n = (int64_t)(rng.next() % (uint64_t)(2 * namp + 1)) - namp;
          v = base + n;
          if (v < lo) v = lo;
          if (v > hi) v = hi;
        }
        img[c][(size_t)y * cw + x] = v;
      }
  }
  return img;
}

// Sets up cs as the options say, up to (not including) write_headers.
// The NLT LUT bytes are kept in luts, which must outlive cs.
inline void configure(ojph::codestream& cs, const EncArgs& e,
                      std::vector<std::vector<uint8_t>>& luts) {
  ojph::param_siz siz = cs.access_siz();
  siz.set_image_extent(ojph::point(e.Xsiz, e.Ysiz));
  siz.set_image_offset(ojph::point(e.iox, e.ioy));
  if (e.have_tile) siz.set_tile_size(ojph::size(e.tw, e.th));
  else siz.set_tile_size(ojph::size(e.Xsiz, e.Ysiz));
  siz.set_tile_offset(ojph::point(e.tox, e.toy));
  siz.set_num_components(e.num_comps);
  for (uint32_t c = 0; c < e.num_comps; ++c)
    siz.set_component(c, ojph::point(e.sub[c].first, e.sub[c].second),
                      e.fmt[c].first, e.fmt[c].second);

  ojph::param_cod cod = cs.access_cod();
  cod.set_num_decomposition(e.levels);
  cod.set_block_dims(e.bw, e.bh);
  if (!e.precincts.empty())
    cod.set_precinct_size((int)e.precincts.size(),
                          const_cast<ojph::size*>(e.precincts.data()));
  cod.set_progression_order(e.prog.c_str());
  cod.set_color_transform(e.rct);
  cod.set_reversible(e.reversible);
  for (const Coc& c : e.cocs) {
    cod.set_num_decomposition(c.comp, c.levels);
    cod.set_block_dims(c.comp, c.bw, c.bh);
    cod.set_reversible(c.comp, e.reversible);
  }
  for (uint32_t c : e.rev_comps) cod.set_reversible(c, true);
  for (uint32_t c : e.irrev_comps) cod.set_reversible(c, false);
  ojph::param_qcd qcd = cs.access_qcd();
  if (e.have_qfactor) qcd.set_qfactor(e.qfactor);
  else if (!e.reversible || !e.irrev_comps.empty()) qcd.set_irrev_quant(e.qstep);
  for (const auto& q : e.qstep_comps) qcd.set_irrev_quant(q.first, q.second);

  ojph::param_nlt nlt = cs.access_nlt();
  for (uint32_t c = 0; c < e.num_comps; ++c) {
    bool want = false;
    if (e.nlt_all && e.fmt[c].second) want = true;
    for (uint32_t n : e.nlt_comps) if (n == c) want = true;
    if (want)
      nlt.set_nonlinear_transform(
        c, ojph::param_nlt::nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT);
  }
  // Each LUT entry takes 1, 2 or 4 bytes, as pt_val (the depth) needs.
  luts.clear();
  luts.reserve(e.nlt4s.size());
  for (const Nlt4& n : e.nlt4s) {
    const uint32_t cdepth = e.fmt[n.comp].first;
    const size_t bpp = cdepth <= 8 ? 1 : (cdepth <= 16 ? 2 : 4);
    std::vector<uint64_t> lut = nlt4_lut(cdepth, n.points);
    std::vector<uint8_t> pts(lut.size() * bpp);
    for (size_t i = 0; i < lut.size(); ++i) {
      if (bpp == 1) pts[i] = (uint8_t)lut[i];
      else if (bpp == 2) { uint16_t v = (uint16_t)lut[i]; memcpy(&pts[i * 2], &v, 2); }
      else { uint32_t v = (uint32_t)lut[i]; memcpy(&pts[i * 4], &v, 4); }
    }
    luts.push_back(pts);
    nlt.set_nonlinear_transform(
      n.comp, (ojph::ui8)cdepth, e.fmt[n.comp].second, 0u, 0xFFFFFFFFu,
      (ojph::ui8)cdepth, (ojph::ui16)n.points, (void*)luts.back().data(),
      ojph::param_nlt::nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT);
  }

  cs.set_planar(e.planar != 0);
  if (e.tlm) cs.request_tlm_marker(true);
  if (e.tp_res || e.tp_comp) cs.set_tilepart_divisions(e.tp_res, e.tp_comp);
}

// The --comment segments, for write_headers.  They point into e.
inline std::vector<ojph::comment_exchange> comments(const EncArgs& e) {
  std::vector<ojph::comment_exchange> coms(e.comments.size());
  for (size_t i = 0; i < e.comments.size(); ++i)
    coms[i].set_string(e.comments[i].c_str());
  return coms;
}

}  // namespace enc_setup

#endif  // ORACLE_ENC_SETUP_H
