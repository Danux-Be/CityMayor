# Road types, road works and roundabouts.

road-gravel = Gravel road
road-street = Two-lane street
road-one-way-single = One-way street (1 lane)
road-one-way = One-way street (2 lanes)
road-avenue = Four-lane avenue
road-boulevard = Boulevard with median
road-boulevard-wide = Six-lane boulevard
road-highway = Highway
road-highway-one-way = Highway carriageway (one-way, 3 lanes)
road-highway-ramp = Highway ramp
road-custom = Custom road
road-roundabout = Roundabout

road-speed-limit = { $speed } km/h
road-under-construction = Under construction ({ $percent }%)
road-opens-in = Opens in { $days ->
    [one] 1 day
   *[other] { $days } days
}

event-roundabout-built = Roundabout built
event-roads-removed = { $count ->
    [one] Road demolished
   *[other] { $count } road sections demolished
}
event-roads-opened = { $count ->
    [one] A road has opened to traffic
   *[other] { $count } roads have opened to traffic
}
