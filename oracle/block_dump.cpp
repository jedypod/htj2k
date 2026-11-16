// block_dump.cpp: wrappers around OpenJPH's generic HT block decoders that
// append one record per call to the file named by $OJPH_BLOCK_DUMP.
//
// build.sh compiles OpenJPH's ojph_block_decoder32.cpp / 64.cpp with
//   -Dojph_decode_codeblock32=ojph_decode_codeblock32_real
//   -Dojph_decode_codeblock64=ojph_decode_codeblock64_real
// so the original functions keep their code under a new name, while this file
// provides the names that ojph_codeblock_fun.cpp stores in its dispatch table
// (the library is built with OJPH_DISABLE_SIMD, so only these generic
// decoders are ever selected).  No OpenJPH source file is modified.
//
// The record layout is documented in tests/data/blocks/README.md.

#include <openjph/ojph_defs.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <vector>

namespace ojph {
namespace local {

bool ojph_decode_codeblock32_real(ui8* coded_data, ui32* decoded_data,
  ui32 missing_msbs, ui32 num_passes, ui32 lengths1, ui32 lengths2,
  ui32 width, ui32 height, ui32 stride, bool stripe_causal);
bool ojph_decode_codeblock64_real(ui8* coded_data, ui64* decoded_data,
  ui32 missing_msbs, ui32 num_passes, ui32 lengths1, ui32 lengths2,
  ui32 width, ui32 height, ui32 stride, bool stripe_causal);

namespace {

std::mutex dump_mutex;

void put_u32(std::vector<ui8>& b, ui32 v) {
  for (int i = 0; i < 4; ++i) b.push_back((ui8)(v >> (8 * i)));
}
void put_u64(std::vector<ui8>& b, ui64 v) {
  for (int i = 0; i < 8; ++i) b.push_back((ui8)(v >> (8 * i)));
}

template <typename T>
void write_record(int bits, const std::vector<ui8>& coded, const T* out,
                  ui32 missing_msbs, ui32 num_passes, ui32 lengths1,
                  ui32 lengths2, ui32 width, ui32 height, ui32 stride,
                  bool stripe_causal, bool result) {
  const char* path = getenv("OJPH_BLOCK_DUMP");
  if (path == NULL || *path == 0) return;
  std::vector<ui8> b;
  b.reserve(40 + coded.size() + (size_t)width * height * sizeof(T));
  b.push_back('O'); b.push_back('J'); b.push_back('B'); b.push_back('K');
  b.push_back((ui8)bits);
  b.push_back(stripe_causal ? 1 : 0);
  b.push_back(result ? 1 : 0);
  b.push_back(0);
  put_u32(b, missing_msbs);
  put_u32(b, num_passes);
  put_u32(b, lengths1);
  put_u32(b, lengths2);
  put_u32(b, width);
  put_u32(b, height);
  put_u32(b, stride);
  put_u32(b, (ui32)coded.size());
  b.insert(b.end(), coded.begin(), coded.end());
  for (ui32 y = 0; y < height; ++y)
    for (ui32 x = 0; x < width; ++x) {
      if (sizeof(T) == 4) put_u32(b, (ui32)out[(size_t)y * stride + x]);
      else put_u64(b, (ui64)out[(size_t)y * stride + x]);
    }
  std::lock_guard<std::mutex> lock(dump_mutex);
  FILE* f = fopen(path, "ab");
  if (!f) { fprintf(stderr, "block_dump: cannot open %s\n", path); abort(); }
  fwrite(b.data(), 1, b.size(), f);
  fclose(f);
}

}  // namespace

bool ojph_decode_codeblock32(ui8* coded_data, ui32* decoded_data,
  ui32 missing_msbs, ui32 num_passes, ui32 lengths1, ui32 lengths2,
  ui32 width, ui32 height, ui32 stride, bool stripe_causal) {
  // Snapshot every argument and the coded bytes before the call.
  std::vector<ui8> coded(coded_data, coded_data + lengths1 + lengths2);
  bool r = ojph_decode_codeblock32_real(coded_data, decoded_data,
    missing_msbs, num_passes, lengths1, lengths2, width, height, stride,
    stripe_causal);
  if (memcmp(coded.data(), coded_data, coded.size()) != 0)
    fprintf(stderr, "block_dump: decoder32 modified its coded input\n");
  write_record(32, coded, decoded_data, missing_msbs, num_passes, lengths1,
               lengths2, width, height, stride, stripe_causal, r);
  return r;
}

bool ojph_decode_codeblock64(ui8* coded_data, ui64* decoded_data,
  ui32 missing_msbs, ui32 num_passes, ui32 lengths1, ui32 lengths2,
  ui32 width, ui32 height, ui32 stride, bool stripe_causal) {
  std::vector<ui8> coded(coded_data, coded_data + lengths1 + lengths2);
  bool r = ojph_decode_codeblock64_real(coded_data, decoded_data,
    missing_msbs, num_passes, lengths1, lengths2, width, height, stride,
    stripe_causal);
  if (memcmp(coded.data(), coded_data, coded.size()) != 0)
    fprintf(stderr, "block_dump: decoder64 modified its coded input\n");
  write_record(64, coded, decoded_data, missing_msbs, num_passes, lengths1,
               lengths2, width, height, stride, stripe_causal, r);
  return r;
}

}  // namespace local
}  // namespace ojph
