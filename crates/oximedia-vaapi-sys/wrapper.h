// Bindgen input for oximedia-vaapi-sys.
//
// libva exposes a single root `va/va.h`; the codec-specific extensions
// (`va_enc_h264.h`, `va_enc_hevc.h`, `va_dec_hevc.h`, `va_vpp.h`) are
// pulled in explicitly so encode-side structures are visible.

#include <va/va.h>
#include <va/va_drm.h>
#include <va/va_x11.h>
#include <va/va_str.h>
#include <va/va_enc_h264.h>
#include <va/va_enc_hevc.h>
#include <va/va_enc_av1.h>
#include <va/va_dec_h264.h>
#include <va/va_dec_hevc.h>
#include <va/va_dec_av1.h>
#include <va/va_vpp.h>
