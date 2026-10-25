# dmgbuild settings for Oyenza Studio's DMG: the window Finder opens when someone opens the DMG. tools/make_mac_dmg.sh
# passes the values below with -D. The window shows the app on the left, a link to Applications on the right, and
# the picture from tools/dmg_art.html behind them, which draws the headline, the arrow and Oyen around those two
# spots: move an icon here and the picture has to follow.
import os.path

app = defines["app"]  # noqa: F821 (dmgbuild supplies `defines`)
name = os.path.basename(app)

files = [app]
symlinks = {"Applications": "/Applications"}
icon = defines.get("icon") or None  # noqa: F821 (the volume's own icon, in Finder's title bar and on the desktop)
background = defines["background"]  # noqa: F821 (a .png with its @2x next to it: dmgbuild makes one HiDPI .tiff)

format = "UDZO"
# No hide_extensions: it writes Finder info onto the app bundle, which breaks `codesign --verify --strict` (and
# notarization). Finder leaves out ".app" by itself.

# The window: its content is the picture's size, 660 x 420 points, under a 28-point title bar that the height
# includes (macOS 15 Finder). No toolbar, sidebar, path bar or status bar.
window_rect = ((200, 140), (660, 448))
default_view = "icon-view"
show_toolbar = False
show_sidebar = False
show_pathbar = False
show_status_bar = False
show_icon_preview = False
show_item_info = False
arrange_by = None
icon_size = 128
text_size = 14
label_pos = "bottom"
# Each icon's centre, in points from the content's top left corner.
icon_locations = {name: (180, 224), "Applications": (480, 224)}
