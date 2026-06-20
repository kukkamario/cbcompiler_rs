// CoolBasic font resolution (FD-018).
//
// Resolves a font *family name* (e.g. "Courier New") to a font file path so
// LoadFont and the default font can be loaded by name. Font arguments that look
// like file paths (they contain a '.') are loaded directly by the caller; this
// maps bare family names only.
//
// Windows uses a static lowercase family->filename table under %WINDIR%\Fonts,
// one map per style (normal/bold/italic/bold-italic), covering the standard
// Windows font set. Linux/other platforms query fontconfig when it is available;
// without it (the default vcpkg/Windows build) resolution returns "".

#include "cb_font.h"

#include <algorithm>
#include <cctype>
#include <cstdlib>
#include <string>
#include <unordered_map>

#ifdef _WIN32

namespace {

// Lowercase family name -> filename within %WINDIR%\Fonts (the standard Windows
// font set). normal / bold / italic / bold-italic variants in the maps below.
const std::unordered_map<std::string, std::string>& normal_fonts() {
    static const std::unordered_map<std::string, std::string> m = {
        {"andalus", "andlso.ttf"},
        {"angsana new", "angsa.ttf"},
        {"angsanaupc", "angsau.ttf"},
        {"arabic transparent", "artro.ttf"},
        {"arial", "arial.ttf"},
        {"arial black", "ariblk.ttf"},
        {"browallia new", "browa.ttf"},
        {"browalliaupc", "browau.ttf"},
        {"comic sans ms", "comic.ttf"},
        {"cordia new", "cordia.ttf"},
        {"cordiaupc", "cordiau.ttf"},
        {"courier new", "cour.ttf"},
        {"david", "david.ttf"},
        {"david transparent", "davidtr.ttf"},
        {"dilleniaupc", "upcdl.ttf"},
        {"estrangelo edessa", "estre.ttf"},
        {"eucrosiaupc", "upcel.ttf"},
        {"fixed miriam transparent", "mriamfx.ttf"},
        {"franklin gothic medium", "framd.ttf"},
        {"frankruehl", "frank.ttf"},
        {"freesiaupc", "upcfl.ttf"},
        {"gautami", "gautami.ttf"},
        {"georgia", "georgia.ttf"},
        {"impact", "impact.ttf"},
        {"irisupc", "upcil.ttf"},
        {"jasmineupc", "upcjl.ttf"},
        {"kodchiangupc", "upckl.ttf"},
        {"latha", "latha.ttf"},
        {"levenim mt", "lvnm.ttf"},
        {"lilyupc", "upcll.ttf"},
        {"lucida console", "lucon.ttf"},
        {"lucida sans unicode", "l_10646.ttf"},
        {"mangal", "mangal.ttf"},
        {"marlett", "marlett.ttf"},
        {"microsoft sans serif", "micross.ttf"},
        {"miriam", "mriam.ttf"},
        {"miriam fixed", "mriamc.ttf"},
        {"miriam transparent", "mriamtr.ttf"},
        {"mv boli", "mvboli.ttf"},
        {"narkisim", "nrkis.ttf"},
        {"palatino linotype", "pala.ttf"},
        {"raavi", "raavi.ttf"},
        {"rod", "rod.ttf"},
        {"rod transparent", "rodtr.ttf"},
        {"shruti", "shruti.ttf"},
        {"simplified arabic", "simpo.ttf"},
        {"simplified arabic fixed", "simpfxo.ttf"},
        {"sylfaen", "sylfaen.ttf"},
        {"symbol", "symbol.ttf"},
        {"tahoma", "tahoma.ttf"},
        {"times new roman", "times.ttf"},
        {"traditional arabic", "trado.ttf"},
        {"trebuchet ms", "trebuc.ttf"},
        {"tunga", "tunga.ttf"},
        {"verdana", "verdana.ttf"},
        {"webdings", "webdings.ttf"},
        {"wingdings", "wingding.ttf"},
        {"simhei", "simhei.ttf"},
        {"fangsong_gb2312", "simfang.ttf"},
        {"dfkai-sb", "kaiu.ttf"},
        {"kaiti_gb2312", "simkai.ttf"},
    };
    return m;
}

const std::unordered_map<std::string, std::string>& bold_fonts() {
    static const std::unordered_map<std::string, std::string> m = {
        {"aharoni", "ahronbd.ttf"},
        {"angsana new", "angsab.ttf"},
        {"angsanaupc", "angsaub.ttf"},
        {"arabic transparent", "artrbdo.ttf"},
        {"arial", "arialbd.ttf"},
        {"browallia new", "browab.ttf"},
        {"browalliaupc", "browaub.ttf"},
        {"comic sans ms", "comicbd.ttf"},
        {"cordia new", "cordiab.ttf"},
        {"cordiaupc", "cordiaub.ttf"},
        {"courier new", "courbd.ttf"},
        {"david", "davidbd.ttf"},
        {"dilleniaupc", "upcdb.ttf"},
        {"eucrosiaupc", "upceb.ttf"},
        {"freesiaupc", "upcfb.ttf"},
        {"georgia", "georgiab.ttf"},
        {"irisupc", "upcib.ttf"},
        {"jasmineupc", "upcjb.ttf"},
        {"kodchiangupc", "upckb.ttf"},
        {"levenim mt", "lvnmbd.ttf"},
        {"lilyupc", "upclb.ttf"},
        {"palatino linotype", "palab.ttf"},
        {"simplified arabic", "simpbdo.ttf"},
        {"tahoma", "tahomabd.ttf"},
        {"times new roman", "timesbd.ttf"},
        {"traditional arabic", "tradbdo.ttf"},
        {"trebuchet ms", "trebucbd.ttf"},
        {"verdana", "verdanab.ttf"},
    };
    return m;
}

const std::unordered_map<std::string, std::string>& italic_fonts() {
    static const std::unordered_map<std::string, std::string> m = {
        {"angsana new", "angsai.ttf"},
        {"angsanaupc", "angsaui.ttf"},
        {"arial", "ariali.ttf"},
        {"browallia new", "browai.ttf"},
        {"browalliaupc", "browaui.ttf"},
        {"cordia new", "cordiai.ttf"},
        {"cordiaupc", "cordiaui.ttf"},
        {"courier new", "couri.ttf"},
        {"dilleniaupc", "upcdi.ttf"},
        {"eucrosiaupc", "upcei.ttf"},
        {"franklin gothic medium", "framdit.ttf"},
        {"freesiaupc", "upcfi.ttf"},
        {"georgia", "georgiai.ttf"},
        {"irisupc", "upcii.ttf"},
        {"jasmineupc", "upcji.ttf"},
        {"kodchiangupc", "upcki.ttf"},
        {"lilyupc", "upcli.ttf"},
        {"palatino linotype", "palai.ttf"},
        {"times new roman", "timesi.ttf"},
        {"trebuchet ms", "trebucit.ttf"},
        {"verdana", "verdanai.ttf"},
    };
    return m;
}

const std::unordered_map<std::string, std::string>& bold_italic_fonts() {
    static const std::unordered_map<std::string, std::string> m = {
        {"angsana new", "angsaz.ttf"},
        {"angsanaupc", "angsauz.ttf"},
        {"arial", "arialbi.ttf"},
        {"browallia new", "browaz.ttf"},
        {"browalliaupc", "browauz.ttf"},
        {"cordia new", "cordiaz.ttf"},
        {"cordiaupc", "cordiauz.ttf"},
        {"courier new", "courbi.ttf"},
        {"dilleniaupc", "upcdbi.ttf"},
        {"eucrosiaupc", "upcebi.ttf"},
        {"freesiaupc", "upcfbi.ttf"},
        {"georgia", "georgiaz.ttf"},
        {"irisupc", "upcibi.ttf"},
        {"jasmineupc", "upcjbi.ttf"},
        {"kodchiangupc", "upckbi.ttf"},
        {"lilyupc", "upclbi.ttf"},
        {"palatino linotype", "palabi.ttf"},
        {"times new roman", "timesbi.ttf"},
        {"trebuchet ms", "trebucbi.ttf"},
        {"verdana", "verdanaz.ttf"},
    };
    return m;
}

// %WINDIR%\Fonts\, or "" if WINDIR is unset (guarded rather than assumed).
const std::string& windows_font_dir() {
    static const std::string dir = [] {
        const char* windir = std::getenv("WINDIR");
        return windir ? std::string(windir) + "\\Fonts\\" : std::string();
    }();
    return dir;
}

}  // namespace

std::string cb::font::find(const char* font, bool is_bold, bool is_italic) {
    if (!font) return std::string();

    std::string name(font);
    std::transform(name.begin(), name.end(), name.begin(),
                   [](unsigned char ch) { return (char)std::tolower(ch); });

    const auto& table = (is_bold && is_italic) ? bold_italic_fonts()
                        : is_bold              ? bold_fonts()
                        : is_italic            ? italic_fonts()
                                               : normal_fonts();

    auto it = table.find(name);
    if (it != table.end()) {
        return windows_font_dir() + it->second;
    }
    return std::string();
}

#else  // !_WIN32

// FONTCONFIG_FOUND is defined by runtime/CMakeLists.txt when find_package
// (Fontconfig) succeeds on a non-Windows build; it then links the library too.
// Without it (fontconfig not installed) this branch compiles out and font
// resolution falls back to Allegro's builtin font — see CMakeLists (FD-022).
#ifdef FONTCONFIG_FOUND
#include <fontconfig/fontconfig.h>

std::string cb::font::find(const char* font, bool is_bold, bool is_italic) {
    if (!font) return std::string();

    FcPattern* pattern = FcPatternBuild(
        nullptr, FC_FAMILY, FcTypeString, font, FC_WEIGHT, FcTypeInteger,
        is_bold ? FC_WEIGHT_BOLD : FC_WEIGHT_REGULAR, FC_SLANT, FcTypeInteger,
        is_italic ? FC_SLANT_ITALIC : FC_SLANT_ROMAN, nullptr);
    if (!pattern) return std::string();
    FcConfigSubstitute(nullptr, pattern, FcMatchPattern);
    FcDefaultSubstitute(pattern);

    std::string path;
    FcResult result;
    FcPattern* matched = FcFontMatch(nullptr, pattern, &result);
    if (matched) {
        FcChar8* file = nullptr;
        if (FcPatternGetString(matched, FC_FILE, 0, &file) == FcResultMatch &&
            file) {
            path.assign(reinterpret_cast<const char*>(file));
        }
        FcPatternDestroy(matched);
    }
    FcPatternDestroy(pattern);
    return path;
}

#else  // !FONTCONFIG_FOUND

std::string cb::font::find(const char* /*font*/, bool /*is_bold*/,
                           bool /*is_italic*/) {
    return std::string();
}

#endif  // FONTCONFIG_FOUND

#endif  // _WIN32
