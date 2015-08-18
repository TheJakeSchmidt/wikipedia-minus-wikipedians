#!/usr/bin/python
"""Parses the instance names from the output of "aws ec2 describe-tags"."""

import json
import sys

json_obj = json.loads(sys.stdin.read())
for tag in json_obj['Tags']:
    print tag['ResourceId']
