"""Télécharge les textures Poly Haven (CC0) utilisées par make_buildings.py.

    python3 tools/blender/fetch_urban_sources.py

Les fichiers (1k, diffuse + normale OpenGL) vont dans
tools/blender/sources/urban/ (ignoré par git : seuls les rendus finaux sont
versionnés, dans crates/render/assets/urban/).
"""
import json
import pathlib
import urllib.request

ASSETS = [
    "brick_wall_001", "painted_plaster_wall", "brown_planks_07", "plaster_grey_04",
    "rectangular_facade_tiles", "corrugated_iron_02", "clay_roof_tiles_02",
    "roof_slates_02", "box_profile_metal_sheet", "gravel_concrete", "asphalt_02",
    "square_concrete_pavers", "concrete_wall_008", "leafy_grass", "gravel_road", "muddy_tracks", "asphalt_04",
]
MAPS = {"Diffuse": "diff", "nor_gl": "nor"}

out = pathlib.Path(__file__).parent / "sources" / "urban"
out.mkdir(parents=True, exist_ok=True)
headers = {"User-Agent": "citymayor-asset-fetch"}
for asset in ASSETS:
    req = urllib.request.Request(f"https://api.polyhaven.com/files/{asset}", headers=headers)
    files = json.load(urllib.request.urlopen(req))
    for key, short in MAPS.items():
        target = out / f"{asset}_{short}.jpg"
        if target.exists():
            continue
        url = files[key]["1k"]["jpg"]["url"]
        data = urllib.request.urlopen(urllib.request.Request(url, headers=headers)).read()
        target.write_bytes(data)
        print(target.name, len(data))
