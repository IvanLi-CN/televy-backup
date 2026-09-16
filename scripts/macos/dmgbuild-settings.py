import json
import os
from pathlib import Path


root_dir = Path(os.environ["TELEVYBACKUP_ROOT_DIR"])
source_dir = Path(os.environ["TELEVYBACKUP_DMG_SOURCE_DIR"])
layout_path = root_dir / "assets/brand/macos/dmg/layout.json"
asset_dir = layout_path.parent
layout = json.loads(layout_path.read_text(encoding="utf-8"))
assert layout["builder"] == {"name": "dmgbuild", "version": "1.6.7"}
assert layout["format"] == "UDZO"

window = layout["window"]
icon_locations = {
    name: tuple(location) for name, location in layout["icon_locations"].items()
}

files = [(str(source_dir / "TelevyBackup.app"), "TelevyBackup.app")]
symlinks = dict(layout["symlinks"])
background_path = Path(os.environ.get("TELEVYBACKUP_DMG_BACKGROUND", str(asset_dir / layout["composed_background"])))
background = str(background_path)

volume_name = os.environ["TELEVYBACKUP_DMG_VOLUME_NAME"]
output = os.environ["TELEVYBACKUP_DMG_OUTPUT"]

assert background_path.is_file(), f"missing composed DMG background: {background}"
assert output, "missing DMG output path"

format = layout["format"]
filesystem = layout["filesystem"]
window_rect = (tuple(window["origin"]), (window["width"], window["height"]))
icon_size = layout["icon_size"]
text_size = 16
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
arrange_by = None
default_view = "icon-view"
