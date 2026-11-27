# Common strings: application, units, dates, numbers.
# Fluent syntax: https://projectfluent.org/fluent/guide/

app-title = CityMayor
app-tagline = Build, zone and govern a living city.

## Numbers

# Séparateur des milliers (en anglais : 1,250,000).
number-group-separator = ,

## Units

# `$formatted` : le nombre déjà mis en forme (séparateur de milliers) ;
# `$count` : la valeur, pour choisir le pluriel.
unit-meters = { $value } m
unit-kilometers = { $value } km
unit-money = ${ $value }
unit-money-signed = { $value ->
    [0] $0
   *[other] { $value } $
}
unit-percent = { $value }%
unit-inhabitants = { $count ->
    [one] { $formatted } inhabitant
   *[other] { $formatted } inhabitants
}
unit-jobs = { $count ->
    [one] { $count } job
   *[other] { $count } jobs
}
unit-buildings = { $count ->
    [one] { $count } building
   *[other] { $count } buildings
}

## Calendar

date-day-month = Day { $day }, month { $month }
date-time = { $hour }:00
date-full = Month { $month }, day { $day }, { $hour }:00

## Generic actions

action-ok = OK
action-cancel = Cancel
action-confirm = Confirm
action-close = Close
action-back = Back
action-undo = Undo
action-redo = Redo
