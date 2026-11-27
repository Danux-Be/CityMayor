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
hud-date-value = Month { $month }, day { $day } · { $hour }:00
hud-demand = Demand
hud-demand-residential = R
hud-demand-commercial = C
hud-demand-industrial = I
hud-speed-pause = Pause
hud-speed-normal = Normal speed
hud-speed-fast = Fast
hud-speed-fastest = Very fast
hud-speed-label-pause = II
hud-speed-label-normal = 1×
hud-speed-label-fast = 2×
hud-speed-label-fastest = 3×
hud-help = Right-drag: rotate · Middle-drag: pan · Wheel: zoom · Space: pause · Esc: cancel

## Tools

tool-roads = Roads
tool-roundabout = Roundabout
tool-zoning = Zoning
tool-bulldoze = Bulldoze
tool-straight = Straight
tool-curved = Curved
tool-roundabout-small = Small
tool-roundabout-medium = Medium
tool-roundabout-large = Large
tool-road-details = { $name } · { $length } m

tool-hint-select = Choose a tool below.
tool-hint-road-start = Click to start the road.
tool-hint-road-end = Click to end the road. Right-click to stop.
tool-hint-curve-control = Click to set the bend of the curve.
tool-hint-roundabout = Click to place the roundabout.
tool-hint-zone = Drag to paint the zone.
tool-hint-bulldoze = Click a road, or drag over buildings.

tool-error-too-short = Too short.
tool-error-water = Roads can't cross water yet.

## Budget panel

hud-budget = Budget
budget-title = City budget
budget-funds = Funds
budget-this-month = This month (estimate)
budget-residential-tax = Residential taxes
budget-business-tax = Business taxes
budget-road-upkeep = Road upkeep
budget-building-upkeep = Building upkeep
budget-net = Balance
budget-last-month = Last month
budget-tax-residential = Residential tax rate
budget-tax-business = Business tax rate
budget-tax-value = { $percent }%
budget-tax-hint = Higher taxes bring in more money but slow the city's growth.
money-signed = { $sign }${ $value }

## Info views

tool-info-views = Info views
overlay-off = Off
legend-low = Low
legend-high = High
legend-served = Served by a road
legend-not-served = Not served
legend-zones = Residential · Commercial · Industrial
legend-empty = { "" }

## Info panel

info-building = Building
info-road = Road
info-type = Type
info-status = Status
info-capacity = Capacity
info-residents = Residents
info-jobs = Jobs
info-land-value = Land value
info-length = Length
info-speed = Speed limit
info-lanes = Lanes
info-lanes-value = { $forward } → · { $backward } ←
info-open = Open to traffic
info-percent = { $percent }%
info-hint = Click a building or a road to see its details.
