# dmgbuild settings — styles the Pomelo installer DMG headlessly (no Finder),
# so it works on CI runners. Invoked from package.sh:
#   dmgbuild -s dmg/dmg_settings.py -D app=<Pomelo.app> "Pomelo" <out.dmg>
import os

app = defines.get("app", "dist/Pomelo.app")
appname = os.path.basename(app)

format = "UDZO"
files = [app]
symlinks = {"Applications": "/Applications"}

background = defines.get("background", "background.png")
window_rect = ((200, 200), (640, 400))
default_view = "icon-view"
icon_size = 128
text_size = 14
label_pos = "bottom"
arrange_by = None

show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False

icon_locations = {
    appname: (160, 165),
    "Applications": (480, 165),
}
