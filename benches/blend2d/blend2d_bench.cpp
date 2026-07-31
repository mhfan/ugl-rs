#include <blend2d/blend2d.h>

#include <algorithm>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <string>
#include <vector>

namespace {
constexpr uint32_t kWidth = 256;
constexpr uint32_t kHeight = 256;
constexpr uint32_t kShapes = 64;

uint32_t argument(int argc, char** argv, const char* name, uint32_t fallback) {
  for (int index = 1; index + 1 < argc; ++index) {
    if (std::strcmp(argv[index], name) == 0)
      return static_cast<uint32_t>(std::strtoul(argv[index + 1], nullptr, 10));
  }
  return fallback;
}

const char* output_path(int argc, char** argv) {
  for (int index = 1; index + 1 < argc; ++index)
    if (std::strcmp(argv[index], "--output") == 0) return argv[index + 1];
  return nullptr;
}

uint64_t checksum(const std::vector<uint8_t>& bytes) {
  uint64_t hash = UINT64_C(0xcbf29ce484222325);
  for (uint8_t byte : bytes) hash = (hash ^ byte) * UINT64_C(0x100000001b3);
  return hash;
}

BLPath scene() {
  BLPath path;
  for (uint32_t index = 0; index < kShapes; ++index) {
    double x = double(index % 8) * 30.0 + 4.25;
    double y = double(index / 8) * 30.0 + 4.5;
    path.move_to(x, y);
    path.line_to(x + 22.5, y);
    path.line_to(x + 22.5, y + 21.75);
    path.line_to(x, y + 21.75);
    path.close();
  }
  return path;
}

bool normalized_rgba(const BLImage& image, std::vector<uint8_t>& output) {
  BLImageData data;
  if (image.get_data(&data) != BL_SUCCESS) return false;
  output.resize(size_t(kWidth) * kHeight * 4);
  for (uint32_t y = 0; y < kHeight; ++y) {
    const uint32_t* source = reinterpret_cast<const uint32_t*>(
        static_cast<const uint8_t*>(data.pixel_data) + intptr_t(y) * data.stride);
    for (uint32_t x = 0; x < kWidth; ++x) {
      uint32_t pixel = source[x];
      size_t offset = (size_t(y) * kWidth + x) * 4;
      output[offset + 0] = uint8_t(pixel >> 16);
      output[offset + 1] = uint8_t(pixel >> 8);
      output[offset + 2] = uint8_t(pixel);
      output[offset + 3] = uint8_t(pixel >> 24);
    }
  }
  return true;
}
}  // namespace

int main(int argc, char** argv) {
  uint32_t warmup = argument(argc, argv, "--warmup", 200);
  uint32_t iterations = argument(argc, argv, "--iterations", 2000);
  uint32_t samples = argument(argc, argv, "--samples", 9);
  if (iterations == 0 || samples == 0) {
    std::fprintf(stderr, "--iterations and --samples must be positive\n");
    return 2;
  }

  BLImage image(kWidth, kHeight, BL_FORMAT_PRGB32);
  BLContext context(image);
  BLPath path = scene();
  context.set_fill_style(BLRgba32(40, 120, 220, 192));
  auto render = [&]() {
    context.clear_all();
    context.fill_path(path);
  };

  for (uint32_t index = 0; index < warmup; ++index) render();
  std::vector<double> timings;
  timings.reserve(samples);
  for (uint32_t sample = 0; sample < samples; ++sample) {
    auto started = std::chrono::steady_clock::now();
    for (uint32_t index = 0; index < iterations; ++index) render();
    auto elapsed = std::chrono::steady_clock::now() - started;
    auto total_ns = std::chrono::duration_cast<std::chrono::nanoseconds>(elapsed).count();
    timings.push_back(double(total_ns) / iterations);
  }
  context.end();
  std::sort(timings.begin(), timings.end());

  std::vector<uint8_t> pixels;
  if (!normalized_rgba(image, pixels)) {
    std::fprintf(stderr, "failed to access Blend2D image data\n");
    return 1;
  }
  if (const char* output = output_path(argc, argv)) {
    std::ofstream file(output, std::ios::binary);
    file.write(reinterpret_cast<const char*>(pixels.data()),
               static_cast<std::streamsize>(pixels.size()));
    if (!file) return 1;
  }

  std::printf("renderer,scene,width,height,samples,iterations,min_ns,median_ns,max_ns,checksum\n");
  std::printf("Blend2D,fill_rectangles_64,%u,%u,%u,%u,%.3f,%.3f,%.3f,%llu\n",
      kWidth, kHeight, samples, iterations, timings.front(), timings[timings.size() / 2],
      timings.back(), static_cast<unsigned long long>(checksum(pixels)));
  return 0;
}
