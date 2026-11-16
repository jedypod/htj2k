// Build (from the htj2k crate root, after oracle/build.sh):
//   C=../reference/OpenJPH/src/core
//   g++ -O1 -g -fexceptions -DOJPH_DISABLE_SIMD -DOJPH_ALIGNED_ALLOC_EXISTS \
//     -I$C -I$C/openjph -I$C/shared -std=c++14 tests/data/params/params_dump.cpp \
//     oracle/build/libojph_oracle.a -o params_dump -lpthread
// Regenerate the expected files (in tests/data/params):
//   params_dump file <the .j2c files of FILES in tests.rs> > files.txt
//   params_dump quant > quant.txt;  params_dump atk > atk.txt
//   params_dump nlt > nlt.txt;      params_dump segs $(cat segs.args) > segs.txt
//
// Prints OpenJPH's view of a codestream's main header (mode "file") or of
// generated quantisation parameters (mode "quant"), for the htj2k params
// unit tests.
#include <cstdio>
#include <cstring>
#include <cstdint>
#include <vector>
#include <string>
#include <stdexcept>
#include "ojph_arch.h"
#include "ojph_file.h"
#include "ojph_mem.h"
#include "ojph_params.h"
#define private public
#include "codestream/ojph_params_local.h"
#undef private

using ojph::ui64; using ojph::ui8; using ojph::ui16; using ojph::ui32; using ojph::si32; using ojph::point; using ojph::size; using ojph::mem_infile; using ojph::mem_outfile; using ojph::infile_base;
using namespace ojph::local;

static ui32 fbits(float f) { ui32 u; memcpy(&u, &f, 4); return u; }

static bool rd16(mem_infile& f, ui16& v) {
  ui8 b[2]; if (f.read(b, 2) != 2) return false; v = (ui16)((b[0] << 8) | b[1]); return true;
}

static void print_derived(param_siz& siz, param_cod& cod, param_qcd& qcd,
                          param_nlt& nlt, param_dfs& dfs) {
  ui32 nc = siz.get_num_components();
  printf("MAGB %u\n", qcd.get_MAGB());
  for (ui32 c = 0; c < nc; ++c) {
    const param_cod* cdp = cod.get_coc(c);
    const param_qcd* qp = qcd.get_qcc(c);
    ui32 nd = cdp->get_num_decompositions();
    const param_dfs* d = NULL;
    if (cdp->is_dfs_defined() && dfs.exists())
      d = dfs.get_dfs(cdp->get_dfs_index());
    bool rev = cdp->access_atk()->is_reversible();
    point rd = siz.get_recon_downsampling(c);
    printf("COMP %u w=%u h=%u rw=%u rh=%u rds=%ux%u nd=%u rev=%d guard=%u prec=%u\n",
      c, siz.get_width(c), siz.get_height(c), siz.get_recon_width(c),
      siz.get_recon_height(c), rd.x, rd.y, nd, rev ? 1 : 0,
      qp->get_num_guard_bits(), qp->propose_precision(cdp));
    printf("KMAX %u", c);
    for (ui32 r = 0; r <= nd; ++r)
      for (ui32 b = (r ? 1 : 0); b <= (r ? 3u : 0u); ++b)
        printf(" %u", qp->get_Kmax(d, nd, r, b));
    printf("\n");
    if (!rev) {
      printf("DELTA %u", c);
      for (ui32 r = 0; r <= nd; ++r)
        for (ui32 b = (r ? 1 : 0); b <= (r ? 3u : 0u); ++b)
          printf(" %08x", fbits(qp->get_irrev_delta(d, nd, c, r, b)));
      printf("\n");
    }
    const nlt_rec* rec = nlt.get_nlt_rec(c);
    if (rec)
      printf("NLTREC %u type=%u bd=%u signed=%d\n", c, rec->get_type(),
        rec->get_bit_depth(), rec->is_signed() ? 1 : 0);
    else
      printf("NLTREC %u none\n", c);
  }
}

static int dump_file(const char* path) {
  FILE* fp = fopen(path, "rb");
  if (!fp) { fprintf(stderr, "cannot open %s\n", path); return 1; }
  std::vector<ui8> data;
  ui8 buf[65536]; size_t n;
  while ((n = fread(buf, 1, sizeof(buf), fp)) > 0) data.insert(data.end(), buf, buf + n);
  fclose(fp);
  mem_infile f; f.open(data.data(), data.size());
  param_siz siz; param_cod cod; param_qcd qcd; param_cap cap; param_nlt nlt;
  param_dfs dfs; param_atk atk;
  ui16 m;
  if (!rd16(f, m) || m != SOC) { fprintf(stderr, "no SOC\n"); return 1; }
  try {
    while (rd16(f, m)) {
      if (m == SIZ) siz.read(&f);
      else if (m == CAP) { cap.read(&f); printf("CAP pcap=%08x\n", cap.Pcap); }
      else if (m == COD) cod.read(&f);
      else if (m == COC) cod.add_coc_object(param_cod::OJPH_COD_UNKNOWN)->read_coc(&f, siz.get_num_components(), &cod);
      else if (m == QCD) qcd.read(&f);
      else if (m == QCC) qcd.add_qcc_object(param_qcd::OJPH_QCD_UNKNOWN)->read_qcc(&f, siz.get_num_components());
      else if (m == NLT) nlt.read(&f);
      else if (m == DFS) dfs.read(&f);
      else if (m == ATK) atk.read(&f);
      else if (m == SOT) break;
      else { ui16 l; rd16(f, l); f.seek(l - 2, infile_base::OJPH_SEEK_CUR); }
    }
    cod.update_atk(&atk);
    siz.link(&cod);
    if (dfs.exists()) siz.link(&dfs);
  } catch (std::exception& e) { printf("ERROR\n"); return 0; }

  printf("SIZ rsiz=%04x x=%u y=%u xo=%u yo=%u xt=%u yt=%u xto=%u yto=%u c=%u\n",
    siz.Rsiz, siz.Xsiz, siz.Ysiz, siz.XOsiz, siz.YOsiz, siz.XTsiz, siz.YTsiz,
    siz.XTOsiz, siz.YTOsiz, siz.Csiz);
  for (ui32 c = 0; c < siz.Csiz; ++c)
    printf("SSIZ %u %02x %u %u depth=%u signed=%d\n", c, siz.cptr[c].SSiz,
      siz.cptr[c].XRsiz, siz.cptr[c].YRsiz, siz.get_bit_depth(c), siz.is_signed(c) ? 1 : 0);
  for (param_cod* p = &cod; p; p = p->next) {
    printf("%s idx=%u scod=%02x prog=%u layers=%u mct=%u nd=%u bw=%u bh=%u bs=%02x wt=%u rev=%d ct=%d",
      p == &cod ? "COD" : "COC", p->comp_idx, p->Scod, p->SGCod.prog_order,
      p->SGCod.num_layers, p->SGCod.mc_trans, p->get_num_decompositions(),
      p->SPcod.block_width, p->SPcod.block_height, p->SPcod.block_style,
      p->SPcod.wavelet_trans, p->is_reversible() ? 1 : 0,
      p->is_employing_color_transform() ? 1 : 0);
    printf(" lps=");
    for (ui32 r = 0; r <= p->get_num_decompositions(); ++r) {
      size s = p->get_log_precinct_size(r); printf("%ux%u,", s.w, s.h);
    }
    printf("\n");
  }
  for (param_qcd* p = &qcd; p; p = p->next) {
    printf("%s idx=%u sqcd=%02x n=%u v=", p == &qcd ? "QCD" : "QCC",
      p->comp_idx, p->Sqcd, p->num_subbands);
    for (ui32 i = 0; i < p->num_subbands; ++i) {
      if ((p->Sqcd & 0x1F) == 0) printf("%02x,", p->SPqcd.u8[i]);
      else printf("%04x,", p->SPqcd.u16[i]);
    }
    printf("\n");
  }
  for (param_nlt* p = &nlt; p; p = p->next)
    printf("NLT c=%u en=%d bd=%02x t=%u np=%u dmin=%u dmax=%u pt=%u\n", p->Cnlt,
      p->enabled ? 1 : 0, p->rec.BDnlt, p->rec.Tnlt, p->rec.num_points,
      p->rec.d_min, p->rec.d_max, p->rec.pt_val);
  print_derived(siz, cod, qcd, nlt, dfs);
  return 0;
}

static void hex(const char* tag, mem_outfile& o) {
  printf("%s ", tag);
  const ui8* d = o.get_data();
  for (size_t i = 0; i < o.tell(); ++i) printf("%02x", d[i]);
  printf("\n");
}

// Each config mirrors one in src/params/tests.rs.
static void quant_config(int k) {
  param_siz siz; param_cod cod; param_qcd qcd; param_cap cap; param_nlt nlt;
  param_dfs dfs; param_atk atk;
  ojph::param_siz S(&siz); ojph::param_cod C(&cod); ojph::param_qcd Q(&qcd);
  ojph::param_nlt N(&nlt);
  ui32 nc = 3, depth = 8, levels = 5; bool sgn = false, rev = true, rct = false;
  point sub[4] = { point(1,1), point(1,1), point(1,1), point(1,1) };
  switch (k) {
    case 0: rct = true; break;                                  // 5/3 rct 8 bit
    case 1: rev = false; depth = 12; levels = 6; rct = true; break; // 9/7 default delta
    case 2: rev = false; depth = 10; sub[1] = sub[2] = point(2,2); break; // qfactor 420
    case 3: rev = false; nc = 1; depth = 16; levels = 2; break;  // weights
    case 4: nc = 3; depth = 16; sgn = true; levels = 3; break;   // COC per comp
    case 5: nc = 4; rev = false; depth = 8; levels = 4; sub[1] = sub[2] = point(2,1); break; // qfactor 422, 4 comps
    case 6: nc = 1; depth = 32; levels = 32; break;              // 32 levels, 32 bit
    case 7: nc = 2; depth = 16; sgn = true; levels = 1; rev = false; break; // nlt 3 + qfactor 1-2 comps
    case 8: case 9: case 10: nc = 3; depth = 16; sgn = true; levels = 2; break; // NLT for ALL_COMPS
  }
  S.set_image_extent(point(67, 45));
  S.set_image_offset(point(3, 1));
  S.set_tile_size(size(67, 45));
  S.set_num_components(nc);
  for (ui32 c = 0; c < nc; ++c)
    S.set_component(c, sub[c], (k == 9 && c == 2) ? 12 : depth, (k == 10 && c == 2) ? false : sgn);
  C.set_num_decomposition(levels);
  C.set_block_dims(32, 32);
  C.set_color_transform(rct);
  C.set_reversible(rev);
  if (k == 1) { C.set_progression_order("LRCP"); size ps[2] = { size(64,64), size(128,128) }; C.set_precinct_size(2, ps); }
  if (k == 2) Q.set_qfactor(85.0f);
  if (k == 3) { float w[7] = { 1.0f, 1.5f, 2.0f, 0.5f, 0.75f, 1.25f, 3.0f }; Q.set_irrev_quant(0.002f, 7, w); }
  if (k == 4) {
    C.set_num_decomposition(1, 2); C.set_block_dims(1, 16, 64);
    C.set_reversible(2, false); Q.set_irrev_quant(2, 0.01f);
    C.set_progression_order("CPRL");
  }
  if (k == 5) { Q.set_qfactor(97.5f); Q.set_qfactor(3, ojph::param_qcd::OJPH_COMP_Y, 60.0f); C.set_progression_order("LRCP"); }
  if (k == 7) { N.set_nonlinear_transform(0, 3); Q.set_qfactor(70.0f); }
  if (k >= 8) N.set_nonlinear_transform(65535, 3);
  if (k == 10) N.set_nonlinear_transform(1, 0);
  try {
    siz.set_cod(cod);
    siz.check_validity();
    cod.check_validity(siz);
    cod.update_atk(&atk);
    qcd.check_validity(siz, cod);
    cap.check_validity(cod, qcd);
    nlt.check_validity(siz, cod);
  } catch (std::exception&) { printf("CONFIG %d ERROR\n", k); return; }
  printf("CONFIG %d\n", k);
  mem_outfile o; o.open();
  siz.write(&o); hex("SIZ", o);
  mem_outfile o2; o2.open(); cap.write(&o2); hex("CAP", o2);
  mem_outfile o3; o3.open(); cod.write(&o3); cod.write_coc(&o3, nc); hex("COD", o3);
  mem_outfile o4; o4.open(); qcd.write(&o4); qcd.write_qcc(&o4, nc); hex("QCD", o4);
  mem_outfile o5; o5.open(); nlt.write(&o5); hex("NLT", o5);
  print_derived(siz, cod, qcd, nlt, dfs);
}

static void dump_atk_dfs() {
  param_atk a;
  param_atk* p97 = a.get_atk(0);
  printf("IRV97 latk=%u satk=%04x k=%08x n=%u", p97->Latk, p97->Satk, fbits(p97->Katk), p97->Natk);
  for (ui32 s = 0; s < p97->Natk; ++s) printf(" %08x", fbits(p97->d[s].irv.Aatk));
  printf("\n");
  param_atk* p53 = a.get_atk(1);
  printf("REV53 latk=%u satk=%04x k=%08x n=%u", p53->Latk, p53->Satk, fbits(p53->Katk), p53->Natk);
  for (ui32 s = 0; s < p53->Natk; ++s)
    printf(" e=%u b=%d a=%d", p53->d[s].rev.Eatk, p53->d[s].rev.Batk, p53->d[s].rev.Aatk);
  printf("\n");
}

// Reads ATK/DFS segment bodies given as hex on the command line.
static void read_segs(int argc, char** argv) {
  param_atk atk; param_dfs dfs;
  for (int i = 2; i + 1 < argc; i += 2) {
    std::string kind = argv[i], h = argv[i + 1];
    std::vector<ui8> b;
    for (size_t j = 0; j + 1 < h.size(); j += 2) b.push_back((ui8)strtol(h.substr(j, 2).c_str(), 0, 16));
    mem_infile f; f.open(b.data(), b.size());
    try {
      if (kind == "atk") atk.read(&f); else dfs.read(&f);
      printf("%s ok\n", kind.c_str());
    } catch (std::exception&) { printf("%s ERROR\n", kind.c_str()); }
  }
  for (param_atk* p = &atk; p; p = p->next) {
    printf("ATK idx=%u latk=%u satk=%04x rev=%d k=%08x n=%u", p->get_index(), p->Latk, p->Satk,
      p->is_reversible() ? 1 : 0, fbits(p->Katk), p->Natk);
    for (ui32 s = 0; s < p->Natk; ++s) {
      if (p->is_reversible()) printf(" e=%u b=%d a=%d", p->d[s].rev.Eatk, p->d[s].rev.Batk, p->d[s].rev.Aatk);
      else printf(" %08x", fbits(p->d[s].irv.Aatk));
    }
    printf("\n");
  }
  for (const param_dfs* p = &dfs; p; p = p->next) {
    printf("DFS s=%u ids=%u", p->Sdfs, p->Ids);
    if (p->Ids == 0) { printf("\n"); continue; } // C++ indexes out of bounds
    for (ui32 l = 1; l <= 8; ++l) printf(" %d", (int)p->get_dwt_type(l));
    point f = p->get_res_downsamp(3);
    printf(" rd3=%ux%u sb=", f.x, f.y);
    for (ui32 r = 1; r <= 4; ++r) for (ui32 s = 1; s <= 3; ++s) printf("%u,", p->get_subband_idx(5, r, s));
    printf("\n");
  }
}

static ui64 fnv_init() { return 0xcbf29ce484222325ull; }
static void fnv(ui64& h, ui32 v) { for (int i = 0; i < 4; ++i) { h ^= (v >> (8 * i)) & 0xFF; h *= 0x100000001b3ull; } }

static void print_rec(const char* tag, const nlt_rec& r) {
  printf("%s t=%u bd=%02x np=%u pt=%u bpp=%u fd=%08x,%08x ft=%08x,%08x delta=%08x inv=%08x mul=%08x enc=%u steps=%u",
    tag, r.Tnlt, r.BDnlt, r.num_points, r.pt_val, r.bytes_per_point,
    fbits(r.fd_min), fbits(r.fd_max), fbits(r.ft_min), fbits(r.ft_max),
    fbits(r.delta), fbits(r.inv_delta), fbits(r.multiplier), r.enc_num_points,
    r.precise_max_steps);
  if (r.dec_points) {
    ui64 h = fnv_init();
    for (si32 i = -1; i <= (si32)r.num_points; ++i) fnv(h, fbits(r.dec_points[i]));
    printf(" dec=%016llx", (unsigned long long)h);
  }
  if (r.approx_enc_points) {
    ui64 h = fnv_init();
    for (si32 i = -2; i <= (si32)r.enc_num_points + 1; ++i) fnv(h, fbits(r.approx_enc_points[i]));
    printf(" approx=%016llx", (unsigned long long)h);
  }
  if (r.precise_enc_dec_indices) {
    ui64 h = fnv_init();
    for (ui32 i = 0; i < r.enc_num_points; ++i) fnv(h, r.precise_enc_dec_indices[i]);
    printf(" idx=%016llx", (unsigned long long)h);
    h = fnv_init();
    for (si32 i = -2; i <= (si32)r.num_points + 1; ++i) fnv(h, fbits(r.precise_dec_lut[i]));
    printf(" lut=%016llx", (unsigned long long)h);
  }
  printf("\n");
}

// LUT nonlinearities: set (encoder tables), write, read back (decoder
// tables). Each case mirrors one in src/params/tests.rs.
static void nlt_lut_case(int k) {
  ui32 np = 0, pt = 8, enc = 0; bool approx = false; ui8 type = 2, bd = 12; bool sg = false;
  ui32 dmin = 1000, dmax = 4000000000u;
  std::vector<ui32> v;
  switch (k) {
    case 0: np = 17; pt = 8; for (ui32 i = 0; i < np; ++i) v.push_back(i * i * 255 / 256); v[0] = 0; for (ui32 i = 1; i < np; ++i) if (v[i] <= v[i-1]) v[i] = v[i-1] + 1; break;
    case 1: np = 100; pt = 14; approx = true; for (ui32 i = 0; i < np; ++i) v.push_back(i * 150 + (i % 7)); break;
    case 2: np = 300; pt = 20; type = 4; sg = true; bd = 30; for (ui32 i = 0; i < np; ++i) v.push_back(i * i * 3 + 5); break;
    case 3: np = 40; pt = 16; enc = 500; approx = false; for (ui32 i = 0; i < np; ++i) v.push_back(1000 + i * 1000 + (i > 20 ? 1 : 0) * 37); break;
    case 4: np = 9; pt = 32; approx = true; enc = 64; for (ui32 i = 0; i < np; ++i) v.push_back(i * 400000000u + 3); break;
    case 5: np = 2; pt = 4; for (ui32 i = 0; i < np; ++i) v.push_back(i * 15); break;
  }
  std::vector<ui8> raw;
  ui8 bpp = nlt_rec::get_bpp(pt);
  for (ui32 x : v) { if (bpp == 1) raw.push_back((ui8)x); else if (bpp == 2) { ui16 t = (ui16)x; raw.insert(raw.end(), (ui8*)&t, (ui8*)&t + 2); } else raw.insert(raw.end(), (ui8*)&x, (ui8*)&x + 4); }
  param_nlt nlt;
  printf("NLTCASE %d\n", k);
  try {
    nlt.set_nonlinear_transform(1, bd, sg, dmin, dmax, (ui8)pt, (ui16)np, raw.data(), type, approx, enc);
  } catch (std::exception&) { printf("SET ERROR\n"); return; }
  print_rec("ENC", nlt.get_nlt_object(1)->rec);
  mem_outfile o; o.open(); nlt.write(&o); hex("NLT", o);
  param_nlt r;
  mem_infile f; f.open(o.get_data() + 2, o.tell() - 2);
  r.read(&f);
  print_rec("DEC", r.get_nlt_object(1)->rec);
}

int main(int argc, char** argv) {
  ojph::set_info_stream(stderr);
  ojph::set_warning_stream(stderr);
  if (argc >= 3 && strcmp(argv[1], "file") == 0) {
    int r = 0;
    for (int i = 2; i < argc; ++i) { printf("FILE %s\n", strrchr(argv[i], '/') ? strrchr(argv[i], '/') + 1 : argv[i]); r |= dump_file(argv[i]); }
    return r;
  }
  if (argc >= 2 && strcmp(argv[1], "quant") == 0) {
    for (int k = 0; k < 11; ++k) quant_config(k);
    return 0;
  }
  if (argc >= 2 && strcmp(argv[1], "atk") == 0) { dump_atk_dfs(); return 0; }
  if (argc >= 2 && strcmp(argv[1], "nlt") == 0) { for (int k = 0; k < 6; ++k) nlt_lut_case(k); return 0; }
  if (argc >= 2 && strcmp(argv[1], "segs") == 0) { read_segs(argc, argv); return 0; }
  fprintf(stderr, "usage\n");
  return 2;
}
