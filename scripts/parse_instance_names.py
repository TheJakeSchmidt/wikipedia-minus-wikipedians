#!/usr/bin/python
"""Parses the instance names from the output of "aws ec2 run-instances"."""

import json
import sys

json_obj = json.loads(sys.stdin.read())
for instance in json_obj['Instances']:
    print instance['InstanceId']
