# Overview

Wikipedia Minus Wikipedians shows what (English) Wikipedia would look like if nobody (and no bots)
were fixing all the vandalism. It does this by finding all instances of vandalism in the page's
revision history, performing a series of 3-way merges with the current contents of the page, and
keeping whatever merges cleanly.

Note that this was mostly a project to help me learn Rust, rather than an attempt to make the next
Instagram. I'm going to write up this README in full anyway, mostly for my own future reference.

I've done Java, Python, and C++, and am somewhat familiar with Lisp and OCaml, so my Rust code
probably looks alternately like each of those languages' typical coding styles. The code is unlikely
to be very idiomatic, and I'm almost certainly violating lots of Rust best practices. I'd happily
accept criticism of the code from Rust veterans, if anyone runs out of things to do. :)

# How to run

## Locally

To run a Wikipedia Minus Wikipedians server locally, listening on port 8888:

    $ cargo build
    $ ./target/debug/wikipedia_minus_wikipedians --port=8888

You can test the server by going to a URL like `http://localhost:8888/wiki/William_Howard_Taft`.

Wikipedia Minus Wikipedians can also use Redis to cache MediaWiki API responses, which speeds up
subsequence loads of the same page and reduces load on Wikipedia. If you have a Redis server running
on `redishost:6379`, you can use it by running:

    $ ./target/debug/wikipedia_minus_wikipedians --redis_hostname redishost --redis_port 6379

For the full list of flags accepted, run:

    $ ./target/debug/wikipedia_minus_wikipedians --help

## On Amazon Web Services

Wikipedia Minus Wikipedians can also be run on AWS. It requires EC2 instances to run the server, the
infrastructure to allow CodeDeploy to deploy to the EC2 instances, and optionally an ElastiCache
cache cluster with one Redis node to cache MediaWiki API responses.

These instructions assume you've run "aws configure", and are acting as an IAM user that has
permissions to create EC2 instances, S3 buckets, CodeDeploy applications and deployment groups, IAM
roles, security groups, EC2 instance profiles, and ElastiCache cache clusters.

First, pick an environment name (e.g. "prod", "QA", "NewFeatureTest"), no longer than 17 characters
or shorter. Then, to bring up the environment (in this case, called NewFeatureTest) with 3
c4.2xlarge EC2 instances (instance type and number of instances are optional):

    $ cd scripts
    $ ./create_environment.sh NewFeatureTest c4.2xlarge 3
    $ ./deploy_revision.sh NewFeatureTest

...and Wikipedia Minus Wikipedians should be available on those EC2 instances! Try out a URL like
`http://ec2-xxx-xxx-xxx-xxx.compute-1.amazonaws.com:3000/wiki/Zachary_Taylor` to test it. To destroy
the environment when you're finished:

    $ ./destroy_environment NewFeatureTest

That should destroy all AWS resources associated with the environment, but if you're worried about
accidental spend, watch the output for errors (and maybe check the AWS console for EC2 instances,
ElastiCache cache clusters, S3 buckets, and/or CodeDeploy applications that weren't deleted).

# Algorithm

How Wikipedia Minus Wikipedians works:

1. Fetch the current contents of the page, `contents`.
1. Fetch the ID and edit summary for the last 500 edits of the page.
1. For every revision whose edit summary contains the string "vandal", assume it represents an
   edit that reverted some vandalism, and:
    1. Fetch the contents of the page at that revision (the clean revision).
    1. Fetch the contents of the page at the previous revision (the vandalized revision).
    1. Attempt a 3-way merge, assuming that the clean revision is the common source, and `contents`
       and the vandalized revision are the two changed texts, and keeping whatever merges in
       (favoring the vandalized revision in the case of conflicts).
    1. Store the merged text in `contents`.
1. Parse the merged wikitext and render it to HTML (using the MediaWiki API on en.wikipedia.org).
1. Fetch the article HTML from Wikipedia, and replace the article body with the rendered HTML from
   the previous step.

## Critical path

This algorithm is parallelized as aggressively as possible, because it is *slow*. Unfortunately, the
sequence of merges is the slowest part, and can't be parallelized (not the way it currently works,
anyway). The critical path of a full page load, which can't be parallelized any further, is:

1. Find the canonical title of the page.
1. Fetch the IDs and edit summaries of the last 500 revisions of the page.
1. Repeatedly:
    1. Fetch the contents of a revision.
    1. Split it up into sections.
    1. Merge a section with the current accumulated contents of that section.
1. Parse the wikitext of the full page, and render to HTML.
1. In the full article HTML, replace the original article body with the rendered wikitext.

The sections are merged in parallel (there is a dedicated merging thread for each section). The
introduction section usually takes the longest to merge.

## Load shedding

As mentioned above, the longest component of a page load is the series of 3-way merges. They can't
be parallelized (the way it currently works, anyway), and they can get very slow.

The 3-way merge starts by doing 2 longest-common-subsequence calculations. This runs in `O(md)`,
where `m` is the length of one of the strings, and `d` is the edit distance between the two
strings. In practice, this means it's worse than quadratic in the edit distance. As the code
attempts to merge revisions farther and farther back in time, organic (non-vandalism) edits pile up,
increasing the edit distance between the old revisions and the current revision. I've seen the time
to do a single longest-common-subsequence calculation reach into the tens of seconds. To avoid that,
the code uses two heuristics to decide to skip some merges entirely:

- The code imposes a hard 500ms timeout on each longest-common-subsequence calculation. Once a
  particular section of the page has hit this timeout for 3 revisions in a row, the code stops
  trying to merge further revisions of that section (because we assume that too many organic edits
  have piled up, and that the page isn't likely to get *more* similar to its current revision if we
  keep going farther back in time).
- If the lengths of the two strings differ by more than 1000 bytes, the code makes no attempt to do
  the merge. I haven't made much effort to tune that number; it definitely skips some massive
  vandalisms that would otherwise just waste time until they hit the timeout.

# Limitations

Wikipedia Minus Wikipedians has some unfortunate limitations, some of which can be dealt with and
some of which can't.

## Merge markers

In order to mark sections of the page that are merged-in vandalism, the merge algorithm puts markers
at the beginning and end of each merged-in section. These markers are done using characters from a
Unicode Private Use Area, which should ensure that no legitimate Wikipedia page contains the merge
markers. That part's easy.

UNFORTUNATELY, sometimes they end up in places (like inside markup) that interfere with page
rendering. Rather than dealing with this properly, so far all I've done is write some regular
expressions that remove markers that are inside HTML tags. This is, obviously, heinous, both because
I'm parsing HTML with regular expressions (<http://stackoverflow.com/a/1732454>), and because this
takes place *after* wikitext parsing. So, for example, when someone vandalizes a page by changing an
image to a different image, and that vandalism gets merged in, the merge marker is part of the image
filename, and so it doesn't render properly (because it doesn't get removed until after wikitext
parsing would generate the `<img>` tag).

## Going backward in time doing 3-way merges is slow and not parallelizable

As the title says. A friend suggested at least one alternate way this could work, which I need to
think more about. The goals here, in descending order of priority, are:

1. The non-vandalized parts of the page should look as close as possible to their current state.
2. As many acts of vandalism should be merged into the page as possible.
3. Pages should load very quickly.

For now, this isn't going to get much better.

## No monitoring

I looked into open-source production monitoring solutions like the ones I'm used to from working at
Google (meaning, arbitrary infrastructure- and application-level timeseries storage and
aggregation). Prometheus (prometheus.io) looks the most promising, but there's no Rust client
library for it (http://prometheus.io/docs/instrumenting/clientlibs/). So, there's no production
monitoring for this until I, or someone else, finds the time to write a Prometheus client library
for Rust.

## Lack of unit tests for a lot of code paths

If Rust has facilities that enable modern unit tests (injecting fake versions of, or
monkey-patching, collaborator objects, to test class behavior hermetically), I can't find
them. Traits might allow this in a similar way as Java interfaces, but they have drawbacks, and I
don't think it would be idiomatic to try to code against traits instead of structs everywhere, just
for the increased testability. So, many code paths in this repository just aren't unit tested at all
- particularly, all the code paths that end with calls to Wikipedia or Redis.

# Example articles

Not all articles have much, or even any, recent vandalism that merges cleanly into the current
article. Many articles are protected from being edited by anonymous users; some articles have
changed too much for the vandalism to stick. Some pages I've found that demonstrate multiple visible
vandalisms:

- `/wiki/Friday_(Rebecca_Black_song)` has its entire first paragraph replaced with a
  JFK-assassination-related conspiracy theory.
- `/wiki/William_Howard_Taft` has a couple of complete, grammatically correct sentences inserted
  below the fold, which is unusual.
- `/wiki/Zachary_Taylor` has a surprisingly diverse set of random vandalisms.
- `/wiki/My_World_2.0` is an article about a Justin Bieber album, and has at least 10 instances of
  vandalism in its first paragraph and infobox, almost all of it homophobic.
