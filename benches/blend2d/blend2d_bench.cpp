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

enum class Operation {
  kFill, kFillClipped, kFillGradient, kFillMasked, kBuildMask, kStroke, kStrokeRound
};

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

const char* scene_name(int argc, char** argv) {
  for (int index = 1; index + 1 < argc; ++index)
    if (std::strcmp(argv[index], "--scene") == 0) return argv[index + 1];
  return "fill_rectangles_64";
}

uint64_t checksum(const std::vector<uint8_t>& bytes) {
  uint64_t hash = UINT64_C(0xcbf29ce484222325);
  for (uint8_t byte : bytes) hash = (hash ^ byte) * UINT64_C(0x100000001b3);
  return hash;
}

BLPath rectangles() {
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

BLPath large_rectangle() {
  BLPath path;
  path.move_to(16.25, 20.5);
  path.line_to(239.5, 20.5);
  path.line_to(239.5, 235.25);
  path.line_to(16.25, 235.25);
  path.close();
  return path;
}

BLPath triangles() {
  BLPath path;
  for (uint32_t index = 0; index < kShapes; ++index) {
    double x = double(index % 8) * 30.0 + 4.25;
    double y = double(index / 8) * 30.0 + 4.5;
    path.move_to(x, y + 21.5);
    path.line_to(x + 11.25, y);
    path.line_to(x + 22.5, y + 21.5);
    path.close();
  }
  return path;
}

BLPath polyline() {
  BLPath path;
  path.move_to(8.0, 128.0);
  for (uint32_t index = 1; index <= 32; ++index) {
    double y = (index & 1) == 0 ? 96.0 : 160.0;
    path.line_to(8.0 + double(index) * 7.5, y);
  }
  return path;
}

BLPath curves() {
  BLPath path;
  path.move_to(8.0, 128.0);
  for (uint32_t index = 0; index < 8; ++index) {
    double x = 8.0 + double(index) * 30.0;
    double y = (index & 1) == 0 ? 112.0 : 144.0;
    path.cubic_to(x + 10.0, y, x + 20.0, y, x + 30.0, 128.0);
  }
  return path;
}

BLPath mask_path() {
  constexpr double k = 55.228474;
  BLPath path;
  path.move_to(228.0, 128.0);
  path.cubic_to(228.0, 128.0 + k, 128.0 + k, 228.0, 128.0, 228.0);
  path.cubic_to(128.0 - k, 228.0, 28.0, 128.0 + k, 28.0, 128.0);
  path.cubic_to(28.0, 128.0 - k, 128.0 - k, 28.0, 128.0, 28.0);
  path.cubic_to(128.0 + k, 28.0, 228.0, 128.0 - k, 228.0, 128.0);
  path.close();
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
  uint32_t warmup = argument(argc, argv, "--warmup", 500);
  uint32_t iterations = argument(argc, argv, "--iterations", 5000);
  uint32_t samples = argument(argc, argv, "--samples", 9);
  if (iterations == 0 || samples == 0) {
    std::fprintf(stderr, "--iterations and --samples must be positive\n");
    return 2;
  }

  const char* scene = scene_name(argc, argv);
  Operation operation = Operation::kFill;
  BLPath path;
  if (std::strcmp(scene, "fill_rectangles_64") == 0) path = rectangles();
  else if (std::strcmp(scene, "fill_rectangle_large") == 0) path = large_rectangle();
  else if (std::strcmp(scene, "fill_rectangle_linear_gradient") == 0) {
    path = large_rectangle();
    operation = Operation::kFillGradient;
  }
  else if (std::strcmp(scene, "fill_rectangle_path_mask") == 0) {
    path = large_rectangle();
    operation = Operation::kFillMasked;
  }
  else if (std::strcmp(scene, "build_path_mask") == 0) {
    path = mask_path();
    operation = Operation::kBuildMask;
  }
  else if (std::strcmp(scene, "fill_triangles_64") == 0) path = triangles();
  else if (std::strcmp(scene, "fill_cubics_8") == 0) path = curves();
  else if (std::strcmp(scene, "fill_cubics_8_clip_rect") == 0) {
    path = curves();
    operation = Operation::kFillClipped;
  } else if (std::strcmp(scene, "stroke_cubics_8") == 0) {
    path = curves();
    operation = Operation::kStroke;
  } else if (std::strcmp(scene, "stroke_polyline_32") == 0) {
    path = polyline();
    operation = Operation::kStroke;
  } else if (std::strcmp(scene, "stroke_polyline_round_32") == 0) {
    path = polyline();
    operation = Operation::kStrokeRound;
  } else {
    std::fprintf(stderr, "unknown scene: %s\n", scene);
    return 2;
  }

  BLImage image(kWidth, kHeight, BL_FORMAT_PRGB32);
  BLContext context(image);
  context.set_fill_style(BLRgba32(40, 120, 220, 192));
  context.set_stroke_style(BLRgba32(40, 120, 220, 192));
  context.set_stroke_width(6.0);
  bool round = operation == Operation::kStrokeRound;
  context.set_stroke_caps(round ? BL_STROKE_CAP_ROUND : BL_STROKE_CAP_BUTT);
  context.set_stroke_join(round ? BL_STROKE_JOIN_ROUND : BL_STROKE_JOIN_MITER_BEVEL);
  context.set_stroke_miter_limit(4.0);
  BLGradient gradient(BLLinearGradientValues(16.0, 128.0, 240.0, 128.0));
  gradient.add_stop(0.0, BLRgba32(0, 0, 0, 32));
  gradient.add_stop(1.0, BLRgba32(0, 0, 0, 224));
  if (operation == Operation::kFillGradient) context.set_fill_style(gradient);
  if (operation == Operation::kBuildMask)
    context.set_fill_style(BLRgba32(255, 255, 255, 255));
  if (operation == Operation::kFillClipped)
    context.clip_to_rect(BLRect(48.0, 104.0, 160.0, 48.0));
  BLImage mask(kWidth, kHeight, BL_FORMAT_PRGB32);
  if (operation == Operation::kFillMasked) {
    BLContext mask_context(mask);
    mask_context.clear_all();
    mask_context.set_fill_style(BLRgba32(255, 255, 255, 255));
    mask_context.fill_path(mask_path());
    mask_context.end();
  }
  auto render = [&]() {
    context.clear_all();
    if (operation == Operation::kFillMasked) {
      context.set_comp_op(BL_COMP_OP_SRC_OVER);
      context.fill_path(path);
      context.set_comp_op(BL_COMP_OP_DST_IN);
      context.blit_image(BLPointI(0, 0), mask);
    } else if (operation == Operation::kStroke || operation == Operation::kStrokeRound)
      context.stroke_path(path);
    else
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
  std::printf("Blend2D,%s,%u,%u,%u,%u,%.3f,%.3f,%.3f,%llu\n", scene,
      kWidth, kHeight, samples, iterations, timings.front(), timings[timings.size() / 2],
      timings.back(), static_cast<unsigned long long>(checksum(pixels)));
  return 0;
}
