// Bindgen input for oximedia-vtb-sys.
//
// We include the umbrella headers for VideoToolbox + the three frameworks it
// transitively requires (CoreMedia for sample/format descriptions, CoreVideo
// for CVPixelBuffer types, CoreFoundation for CF refcounting), plus
// AudioToolbox for AAC encode/decode via AudioConverter.

#include <CoreFoundation/CoreFoundation.h>
#include <CoreVideo/CoreVideo.h>
#include <CoreMedia/CoreMedia.h>
#include <VideoToolbox/VideoToolbox.h>
#include <AudioToolbox/AudioToolbox.h>
