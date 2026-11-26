# Names of simulation concepts: zones, buildings, overlays, statistics, events.

## Zones

zone-unzoned = Unzoned
zone-residential = Residential
zone-commercial = Commercial
zone-industrial = Industrial

## Building types

building-kind-household = Housing
building-kind-shop = Shop
building-kind-factory = Factory

## Overlays

overlay-zone = Zoning
overlay-land-value = Land value
overlay-road-access = Road access
overlay-population = Population density
overlay-traffic = Traffic
overlay-noise = Noise
overlay-pollution = Pollution
overlay-service-coverage = Service coverage
overlay-electricity = Electricity
overlay-water = Water

## City statistics

stat-tick = Tick
stat-population = Population
stat-housing-capacity = Housing
stat-jobs = Jobs
stat-unemployed = Unemployed
stat-happiness = Happiness
stat-demand-residential = Res. demand
stat-demand-commercial = Com. demand
stat-demand-industrial = Ind. demand
stat-funds = Funds
stat-buildings = Buildings
stat-road-segments = Road segments
stat-average-land-value = Average land value

## Events

event-zone-applied = { $changed ->
    [one] 1 tile zoned as { $zone }
   *[other] { $changed } tiles zoned as { $zone }
}
event-bulldozed = { $removed ->
    [one] 1 building demolished
   *[other] { $removed } buildings demolished
}
event-road-built = Road built ({ $length } m)
event-building-built = Construction started: { $kind } ({ $capacity } capacity)
event-building-completed = { $kind } completed

## Construction stages

construction-site-preparation = Site preparation
construction-foundations = Foundations
construction-structure = Structural work
construction-finishing = Finishing works
construction-complete = Completed
construction-progress = { $stage } ({ $percent }%)
event-household-emptied = A home was abandoned
event-budget-settled = Budget for month { $month }: +${ $income } / -${ $expenses } → ${ $funds }
