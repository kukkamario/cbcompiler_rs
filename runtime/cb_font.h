#ifndef CB_FONT_H
#define CB_FONT_H

#include <string>

// System font-name resolution (FD-018).
//
// Given a font *family name* such as "Courier New", returns an absolute path to
// a matching `.ttf`, or "" when no match is found. The caller (LoadFont / the
// default-font loader in cb_gfx.cpp) handles file-path font arguments directly;
// this only maps bare family names.
//
//  * Windows: a static lowercase family->filename table under %WINDIR%\Fonts,
//    with separate normal / bold / italic / bold-italic variants.
//  * Other platforms: a fontconfig query, guarded by FONTCONFIG_FOUND. Without
//    fontconfig it returns "" (the runtime currently builds on Windows/vcpkg).
namespace cb::font {
std::string find(const char* font, bool is_bold = false, bool is_italic = false);
}  // namespace cb::font

#endif  // CB_FONT_H
