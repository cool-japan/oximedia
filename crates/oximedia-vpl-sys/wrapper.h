// Bindgen input for oximedia-vpl-sys.
//
// Intel oneVPL ships a single umbrella `vpl/mfx.h` (the legacy Media SDK
// path) and the newer `vpl/mfxvideo.h`. We include the umbrella to pick
// up structures + the C function table for runtime dispatch.

#include "vpl/mfx.h"
#include "vpl/mfxvideo.h"
#include "vpl/mfxdispatcher.h"
