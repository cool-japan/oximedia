// Bindgen input for oximedia-amf-sys.
//
// The AMD Advanced Media Framework is a C++ SDK, but it ships a pure-C
// dispatch surface that FFmpeg uses to drive VCE/VCN encode and decode.
// We expose only the C-ABI surface here; the C++ object model is opaque.

#include "core/Factory.h"
#include "core/Result.h"
#include "core/Variant.h"
#include "components/VideoEncoderVCE.h"
#include "components/VideoEncoderHEVC.h"
#include "components/VideoEncoderAV1.h"
#include "components/VideoDecoderUVD.h"
