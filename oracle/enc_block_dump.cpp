// enc_block_dump.cpp: wrappers around OpenJPH's generic HT block encoders
// that append one record per call to the file named by $OJPH_ENC_BLOCK_DUMP.
//
// build.sh compiles OpenJPH's ojph_block_encoder.cpp with
//   -Dojph_encode_codeblock32=ojph_encode_codeblock32_real
//   -Dojph_encode_codeblock64=ojph_encode_codeblock64_real
// so the original functions keep their code under a new name, while this file
// provides the names that ojph_codeblock_fun.cpp stores in its dispatch table
// (the library is built with OJPH_DISABLE_SIMD, so only these generic
// encoders are ever selected).  No OpenJPH source file is modified.  This is
// the encoder counterpart of block_dump.cpp; it uses its own environment
// variable and record magic (OJBE), so encoder and decoder records never mix.
//
// A call that raises OJPH_ERROR (one of the encoder's fixed-size buffers is
// full) throws.  The wrapper then writes a record with status 1 and the error
// code held in block_dump_last_error, and rethrows.  OpenJPH's default error
// handler does not pass the code on; enc_blocks_synth.cpp installs one that
// sets block_dump_last_error.  With the default handler the code is 0.
//
// The record layout is documented in tests/data/encoder/README.md.

#include <openjph/ojph_defs.h>
#include <openjph/ojph_mem.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <vector>

ojph::ui32 block_dump_last_error = 0;

namespace ojph {
namespace local {

void ojph_encode_codeblock32_real(ui32* buf, ui32 missing_msbs,
  ui32 num_passes, ui32 width, ui32 height, ui32 stride, ui32* lengths,
  mem_elastic_allocator* elastic, coded_lists*& coded);
void ojph_encode_codeblock64_real(ui64* buf, ui32 missing_msbs,
  ui32 num_passes, ui32 width, ui32 height, ui32 stride, ui32* lengths,
  mem_elastic_allocator* elastic, coded_lists*& coded);

namespace {

std::mutex dump_mutex;

void put_u32(std::vector<ui8>& b, ui32 v) {
  for (int i = 0; i < 4; ++i) b.push_back((ui8)(v >> (8 * i)));
}
void put_u64(std::vector<ui8>& b, ui64 v) {
  for (int i = 0; i < 8; ++i) b.push_back((ui8)(v >> (8 * i)));
}

template <typename T>
void write_record(int bits, const std::vector<T>& in, ui32 missing_msbs,
                  ui32 num_passes, ui32 width, ui32 height, ui32 stride,
                  bool failed, ui32 error_code, ui32 length0,
                  const ui8* coded, ui32 coded_len) {
  const char* path = getenv("OJPH_ENC_BLOCK_DUMP");
  if (path == NULL || *path == 0) return;
  std::vector<ui8> b;
  b.reserve(40 + in.size() * sizeof(T) + coded_len);
  b.push_back('O'); b.push_back('J'); b.push_back('B'); b.push_back('E');
  b.push_back((ui8)bits);
  b.push_back(failed ? 1 : 0);
  b.push_back(0);
  b.push_back(0);
  put_u32(b, missing_msbs);
  put_u32(b, num_passes);
  put_u32(b, width);
  put_u32(b, height);
  put_u32(b, stride);
  put_u32(b, length0);
  put_u32(b, coded_len);
  put_u32(b, error_code);
  for (size_t i = 0; i < in.size(); ++i) {
    if (sizeof(T) == 4) put_u32(b, (ui32)in[i]);
    else put_u64(b, (ui64)in[i]);
  }
  if (coded_len) b.insert(b.end(), coded, coded + coded_len);
  std::lock_guard<std::mutex> lock(dump_mutex);
  FILE* f = fopen(path, "ab");
  if (!f) { fprintf(stderr, "enc_block_dump: cannot open %s\n", path); abort(); }
  fwrite(b.data(), 1, b.size(), f);
  fclose(f);
}

// The height x width samples the encoder may read, row by row, without the
// stride padding.
template <typename T>
std::vector<T> snapshot(const T* buf, ui32 width, ui32 height, ui32 stride) {
  std::vector<T> v;
  v.reserve((size_t)width * height);
  for (ui32 y = 0; y < height; ++y)
    for (ui32 x = 0; x < width; ++x) v.push_back(buf[(size_t)y * stride + x]);
  return v;
}

template <typename T, typename F>
void encode_and_record(int bits, F real, T* buf, ui32 missing_msbs,
                       ui32 num_passes, ui32 width, ui32 height, ui32 stride,
                       ui32* lengths, mem_elastic_allocator* elastic,
                       coded_lists*& coded) {
  std::vector<T> in = snapshot(buf, width, height, stride);
  block_dump_last_error = 0;
  try {
    real(buf, missing_msbs, num_passes, width, height, stride, lengths,
         elastic, coded);
  } catch (...) {
    write_record(bits, in, missing_msbs, num_passes, width, height, stride,
                 true, block_dump_last_error, 0, NULL, 0);
    throw;
  }
  if (snapshot(buf, width, height, stride) != in)
    fprintf(stderr, "enc_block_dump: encoder%d modified its input\n", bits);
  if (coded->avail_size != 0 || coded->buf_size != lengths[0])
    fprintf(stderr, "enc_block_dump: encoder%d left unexpected sizes\n", bits);
  write_record(bits, in, missing_msbs, num_passes, width, height, stride,
               false, 0, lengths[0], coded->buf, lengths[0]);
}

}  // namespace

void ojph_encode_codeblock32(ui32* buf, ui32 missing_msbs, ui32 num_passes,
  ui32 width, ui32 height, ui32 stride, ui32* lengths,
  mem_elastic_allocator* elastic, coded_lists*& coded) {
  encode_and_record<ui32>(32, ojph_encode_codeblock32_real, buf,
    missing_msbs, num_passes, width, height, stride, lengths, elastic, coded);
}

void ojph_encode_codeblock64(ui64* buf, ui32 missing_msbs, ui32 num_passes,
  ui32 width, ui32 height, ui32 stride, ui32* lengths,
  mem_elastic_allocator* elastic, coded_lists*& coded) {
  encode_and_record<ui64>(64, ojph_encode_codeblock64_real, buf,
    missing_msbs, num_passes, width, height, stride, lengths, elastic, coded);
}

}  // namespace local
}  // namespace ojph
