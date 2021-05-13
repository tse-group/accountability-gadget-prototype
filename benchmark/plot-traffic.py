#! /usr/bin/env python3

import os
import sys
import re

assert(len(sys.argv) == 2)


vals = []

f = open(sys.argv[1], 'r')
for l in f.readlines()[2:]:
    l = l.strip()
    while '  ' in l:
        l = l.replace('  ', ' ')

    (t, rx, tx, rx_total, tx_total) = l.split(" ")
    if rx != rx_total or tx != tx_total:
        print(rx, rx_total, tx, tx_total)
        assert(False)

    t = t.split(':')
    t = int(t[0])*3600 + int(t[1])*60 + int(t[2])

    vals.append((t, rx, tx))


min_time = min([ x[0] for x in vals ])

vals = [ (x[0] - min_time, x[1], x[2]) for x in vals ]

print("t", "rx", "tx")
for (t, rx, tx) in vals: #[::4]:
    print(t, rx, tx)
