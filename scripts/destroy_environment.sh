#!/bin/bash

# Destroys an AWS environment that was created by create_environment.sh. This should do the opposite
# of each action in that file, in reverse order.
#
# Usage: destroy_environment.sh <environment>

environment_name=$1

echo Removing S3 bucket Wikipedia-Minus-Wikipedians-Revisions-$environment_name...
aws s3 rm s3://Wikipedia-Minus-Wikipedians-Revisions-$environment_name --recursive
aws s3 rb s3://Wikipedia-Minus-Wikipedians-Revisions-$environment_name

for instance_id in $(aws ec2 describe-tags --filters Name=key,Values=WikipediaMinusWikipediansEnvironment Name=value,Values=$environment_name Name=resource-type,Values=instance | ./parse_tags.py)
do
    echo Terminating EC2 instance $instance_id...
    aws ec2 terminate-instances --instance-ids $instance_id
done

for instance_id in $(aws ec2 describe-tags --filters Name=key,Values=WikipediaMinusWikipediansEnvironment Name=value,Values=$environment_name Name=resource-type,Values=instance | ./parse_tags.py)
do
    echo Waiting for instance $instance_id to be terminated...
    while [ "$(aws ec2 describe-instance-status --instance-ids $instance_id | ./parse_instance_state.py)" != "terminated" ] && [ "$(aws ec2 describe-instance-status --instance-ids $instance_id | ./parse_instance_state.py)" != "unknown" ]
    do
	sleep 1
	echo Waiting for instance $instance_id to be terminated...
    done
done

echo Deleting EC2 security group wikipedia-minus-wikipedians-$environment_name...
aws ec2 delete-security-group --group-name wikipedia-minus-wikipedians-$environment_name

echo Deleting ElastiCache cache cluster WMW$environment_name...
aws elasticache delete-cache-cluster --cache-cluster-id WMW$environment_name
echo Waiting for ElastiCache cache cluster WMW$environment_name to be deleted...
aws elasticache wait cache-cluster-deleted --cache-cluster-id WMW$environment_name

echo Deleting security group wikipedia-minus-wikipedians-elasticache-$environment_name
aws ec2 delete-security-group --group-name wikipedia-minus-wikipedians-elasticache-$environment_name

echo Deleting CodeDeploy deployment group WikipediaMinusWikipedians$(echo $environment_name)...
aws deploy delete-deployment-group --application-name WikipediaMinusWikipedians$(echo $environment_name) --deployment-group-name WikipediaMinusWikipedians$(echo $environment_name)
echo Deleting CodeDeploy application WikipediaMinusWikipedians$(echo $environment_name)...
aws deploy delete-application --application-name WikipediaMinusWikipedians$(echo $environment_name)

echo Detaching AWSCodeDeployRole policy from service role Wikipedia-Minus-Wikipedians-$environment_name-CodeDeploy...
aws iam detach-role-policy --role-name Wikipedia-Minus-Wikipedians-$environment_name-CodeDeploy --policy-arn arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole
echo Deleting service role Wikipedia-Minus-Wikipedians-$environment_name-CodeDeploy...
aws iam delete-role --role-name Wikipedia-Minus-Wikipedians-$environment_name-CodeDeploy

echo Removing role Wikipedia-Minus-Wikipedians-$environment_name-EC2 from EC2 instance profile Wikipedia-Minus-Wikipedians-$environment_name-EC2-Instance-Profile...
aws iam remove-role-from-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-$environment_name-EC2-Instance-Profile --role-name Wikipedia-Minus-Wikipedians-$environment_name-EC2
echo Deleting EC2 instance profile Wikipedia-Minus-Wikipedians-$environment_name-EC2-Instance-Profile...
aws iam delete-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-$environment_name-EC2-Instance-Profile
echo Deleting inline policy WikipediaMinusWikipedians$(echo $environment_name)EC2 from IAM role Wikipedia-Minus-Wikipedians-$environment_name-EC2...
aws iam delete-role-policy --role-name Wikipedia-Minus-Wikipedians-$environment_name-EC2 --policy-name WikipediaMinusWikipedians$(echo $environment_name)EC2
echo Deleting IAM role Wikipedia-Minus-Wikipedians-$environment_name-EC2...
aws iam delete-role --role-name Wikipedia-Minus-Wikipedians-$environment_name-EC2
