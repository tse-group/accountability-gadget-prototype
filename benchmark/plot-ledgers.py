#! /usr/bin/env python3

import os
import sys
import re

assert(len(sys.argv) == 2)

LOGacc = []
LOGava = []

f = open(sys.argv[1], 'r')
for l in f.readlines():
    for (LOG, regex) in [ (LOGacc, r"(\d\d)T(\d\d):(\d\d):(\d\d.\d\d\d)Z INFO  mynode::lcblocktreemanager] Current LOGacc: (\d+) "), (LOGava, r"(\d\d)T(\d\d):(\d\d):(\d\d.\d\d\d)Z INFO  mynode::lcblocktreemanager] Current LOGava: (\d+) "), ]:
        m = re.findall(regex, l)
        if m:
            (d, h, m, s, v) = m[0]
            (d, h, m, v) = map(int, (d, h, m, v))
            s = float(s)
            dt = d * 86400 + h * 3600 + m * 60 + s
            LOG.append((dt, v))

min_time = min([ x[0] for x in LOGacc ])
min_time = min(min_time, min([ x[0] for x in LOGava ]))

LOGacc = [ (x[0] - min_time, x[1]) for x in LOGacc ]
LOGava = [ (x[0] - min_time, x[1]) for x in LOGava ]

print("t", "LOGacc", "LOGava")
for ((t1, acc), (t2, ava)) in zip(LOGacc, LOGava):
    # print(t1, t2)
    if not abs(t1-t2) < 1:
        print(t1, t2)
        assert(abs(t1-t2) < 1)
    print(t1, acc, ava)
    
# print("LOGacc:", " ".join(str(x) for x in LOGacc))
# print("LOGava:", " ".join(str(x) for x in LOGava))
