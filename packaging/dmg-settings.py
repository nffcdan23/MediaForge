# Layout of the MediaForge disk image, for dmgbuild. Invoked by scripts/package-macos.sh, which supplies the
# values read from `defines`. Icon positions must match scripts/make_dmg_background.swift.
import os

app = defines["app"]
internal = defines.get("internal") == "1"
readme = defines.get("readme")

format = "ULMO"                      # LZMA: smallest download; readable by every macOS that can run the app
files = [app] + ([readme] if internal and readme else [])
symlinks = {"Applications": "/Applications"}
icon = defines["volume_icon"]
background = defines["background"]

default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
include_icon_view_settings = True
include_list_view_settings = False

window_rect = ((240, 140), (680, 560 if internal else 430))
icon_size = 104
text_size = 13
arrange_by = None
label_pos = "bottom"

icon_locations = {
    os.path.basename(app): (180, 200),
    "Applications": (500, 200),
}
if internal and readme:
    icon_locations[os.path.basename(readme)] = (340, 456)
