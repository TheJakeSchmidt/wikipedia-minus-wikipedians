#!/usr/bin/python
"""Parses the instance states from the output of "aws ec2 describe-instance-status"."""

import json
import sys

try:
    json_obj = json.loads(sys.stdin.read())
    if json_obj['InstanceStatuses']:
        for tag in json_obj['InstanceStatuses']:
            print tag['InstanceState']['Name']
    else:
        print 'unknown'
except:
    print 'unknown'
