#!/bin/bash

# Deploys a revision to a Wikipedia Without Wikipedians environment.

# TODO: This currently only deploys to an environment called "QA". Generalize it.

echo Creating revision directory in /tmp/wikipedia-minus-wikipedians-revision...
rm -r /tmp/wikipedia-minus-wikipedians-revision
mkdir /tmp/wikipedia-minus-wikipedians-revision
cp ../target/debug/wikipedia_minus_wikipedians ../log.toml appspec.yml start_service.sh \
    stop_service.sh wikipedia-minus-wikipedians.conf \
    /tmp/wikipedia-minus-wikipedians-revision

echo Pushing revision to S3 bucket Wikipedia-Minus-Wikipedians-Revisions-QA...
aws deploy push --application-name WikipediaMinusWikipediansQA --description "Test revision 2" --ignore-hidden-files --s3-location s3://Wikipedia-Minus-Wikipedians-Revisions-QA/wikipedia-minus-wikipedians.zip --source /tmp/wikipedia-minus-wikipedians-revision/
echo Creating deployment...
aws deploy create-deployment --application-name WikipediaMinusWikipediansQA --s3-location bucket=Wikipedia-Minus-Wikipedians-Revisions-QA,key=wikipedia-minus-wikipedians.zip,bundleType=zip --deployment-group-name WikipediaMinusWikipediansQA --deployment-config-name CodeDeployDefault.OneAtATime --description "Deployment started at $(date)"
