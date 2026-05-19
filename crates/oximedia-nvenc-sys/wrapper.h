// Bindgen input for oximedia-nvenc-sys.
//
// The NVIDIA Video Codec SDK ships `nvEncodeAPI.h` and `cuviddec.h` /
// `nvcuvid.h`. We bring them all in so encode (NVENC) and decode (NVDEC)
// share the bindings. The SDK install path is supplied via `NV_CODEC_SDK`
// at build time.

#include "nvEncodeAPI.h"
#include "cuviddec.h"
#include "nvcuvid.h"
