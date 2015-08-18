#!/usr/bin/python
"""Parses the security group IDs from the output of "aws ec2 describe-security-groups"."""

import json
import sys

try:
    json_obj = json.loads(sys.stdin.read())
    if json_obj['SecurityGroups']:
        for security_group in json_obj['SecurityGroups']:
            print security_group['GroupId'] or 'unknown'
    else:
        print 'unknown'
except:
    print 'unknown'
