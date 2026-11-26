# Interface: window, HUD, tools, demo output.

window-title = CityMayor

## Headless demonstration (citymayor binary)

demo-map-info = Map { $width }×{ $height } tiles of { $tile } m · { $hz } Hz ticks · { $ticks } ticks per day
demo-budget-line = $ Month { $month } budget: +{ $income } / -{ $expenses } = { $net } → { $funds } $
demo-rejected = ! Action refused: { $reason }
demo-summary = Buildings: { $buildings } · road segments: { $segments } · average land value (zoned): { $value }
demo-calendar = Days elapsed: { $day } · month: { $month } · hour: { $hour }:00
demo-overlays-ready = Overlays available: { $list }
demo-overlays-planned = Overlays coming later: { $list }
demo-tile-sample = Sample tile { $tile }: zone { $zone } · land value { $value } · road access { $access }

## HUD

hud-population = Population
hud-funds = Funds
hud-date = Date
hud-demand = Demand
hud-speed-pause = Pause
hud-speed-normal = Normal speed
hud-speed-fast = Fast
hud-speed-fastest = Very fast

## Tools

tool-zone = Zoning
tool-road = Roads
tool-bulldoze = Bulldoze
tool-overlays = Info views
tool-zone-paint = Paint { $zone }
