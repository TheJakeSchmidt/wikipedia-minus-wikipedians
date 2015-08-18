#!/usr/bin/python
"""Parses the cache node endpoint addresses from the output of "aws elasticache describe-cache-clusters --show-cache-node-info"."""

import json
import sys

try:
    json_obj = json.loads(sys.stdin.read())
    if json_obj['CacheClusters']:
        for cluster in json_obj['CacheClusters']:
            if cluster['CacheNodes']:
                for cache_node in cluster['CacheNodes']:
                    print cache_node['Endpoint']['Address']
            else:
                print 'unknown'
    else:
        print 'unknown'
except:
    print 'unknown'
