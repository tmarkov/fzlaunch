#!/usr/bin/env python
from math import *
import sys
import os

SEP = os.environ['SEP']

if __name__ == "__main__":
    try:
        query = sys.argv[1].split("=")[0]
        res = eval(query)
        print(f"echo{SEP}{res}{SEP};t {query} = {res}")
    except:
        pass

