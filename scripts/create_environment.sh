#!/bin/bash

# Creates an AWS environment (QA, prod, test, etc.) for running and deploying Wikipedia Without
# Wikipedians. An environment consists of an EC2 instance profile (with an associated IAM role and
# policy), a service role for CodeDeploy, a COdeDeploy application and deployment group, a set of
# EC2 instances, and an S3 bucket to hold code revisions. The name of every resource created by this
# script contains the environment name, so that many environments can coexist in isolation from each
# other.
#
# The environment name must be no longer than 17 characters.
#
# Usage: create_environment.sh <environment> [<instance type> [<number of instances>]]

if [ "$#" -lt "1" ] || [ "$#" -gt "3" ]
then
  echo Wrong number of arguments.
  echo "Usage: $0 <environment> [<instance type> [<number of instances>]]"
  exit
fi

environment_name=$1

if [ "$#" -gt "1" ]
then
  instance_type=$2
else
  instance_type="t2.micro"
fi

if [ "$#" -gt "2" ]
then
  instances=$3
else
  instances=1
fi

# Create IAM instance profile
# Created based on the instructions at
# http://docs.aws.amazon.com/codedeploy/latest/userguide/how-to-create-iam-instance-profile.html.
echo Creating IAM role Wikipedia-Minus-Wikipedians-$environment_name-EC2...
aws iam create-role --role-name Wikipedia-Minus-Wikipedians-$environment_name-EC2 --assume-role-policy-document file://Wikipedia-Minus-Wikipedians-CodeDeploy-EC2-Trust.json
echo Attaching inline policy WikipediaMinusWikipedians$(echo $environment_name)EC2 to IAM role Wikipedia-Minus-Wikipedians-$environment_name-EC2...
aws iam put-role-policy --role-name Wikipedia-Minus-Wikipedians-$environment_name-EC2 --policy-name WikipediaMinusWikipedians$(echo $environment_name)EC2 --policy-document file://Wikipedia-Minus-Wikipedians-CodeDeploy-EC2-Permissions.json
echo Creating EC2 instance profile Wikipedia-Minus-Wikipedians-$environment_name-EC2-Instance-Profile...
aws iam create-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-$environment_name-EC2-Instance-Profile
echo Adding IAM role Wikipedia-Minus-Wikipedians-$environment_name-EC2 to EC2 instance profile Wikipedia-Minus-Wikipedians-$environment_name-EC2-Instance-Profile...
aws iam add-role-to-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-$environment_name-EC2-Instance-Profile --role-name Wikipedia-Minus-Wikipedians-$environment_name-EC2

# Create a service role for AWS CodeDeploy
echo Creating IAM service role Wikipedia-Minus-Wikipedians-$environment_name-CodeDeploy...
aws iam create-role --role-name Wikipedia-Minus-Wikipedians-$environment_name-CodeDeploy --assume-role-policy-document file://Wikipedia-Minus-Wikipedians-CodeDeploy-Trust.json
echo Attaching IAM policy AWSCodeDeployRole to service role Wikipedia-Minus-Wikipedians-$environment_name-CodeDeploy...
aws iam attach-role-policy --role-name Wikipedia-Minus-Wikipedians-$environment_name-CodeDeploy --policy-arn arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole

# There's a small delay before the new service role can be used to create a CodeDeploy deployment group.
echo Sleeping for 10 seconds...
sleep 10

# Create an AWS CodeDeploy application and deployment group
echo Creating CodeDeploy application WikipediaMinusWikipedians$(echo $environment_name)...
aws deploy create-application --application-name WikipediaMinusWikipedians$(echo $environment_name)
echo Creating CodeDeploy deployment group WikipediaMinusWikipedians$(echo $environment_name)...
aws deploy create-deployment-group --application-name WikipediaMinusWikipedians$(echo $environment_name) --deployment-group-name WikipediaMinusWikipedians$(echo $environment_name) --service-role-arn $(aws iam get-role --role-name Wikipedia-Minus-Wikipedians-$environment_name-CodeDeploy --query "Role.Arn" --output text) --ec2-tag-filters Key=WikipediaMinusWikipediansEnvironment,Value=$environment_name,Type=KEY_AND_VALUE

echo Creating security group wikipedia-minus-wikipedians-elasticache-$environment_name
aws ec2 create-security-group --group-name wikipedia-minus-wikipedians-elasticache-$environment_name --description "Security group for ElastiCache cluster in Wikipedia Minus Wikipedians environment $environment_name"
echo Authorizing inbound Redis for security group wikipedia-minus-wikipedians-elasticache-$environment_name...
aws ec2 authorize-security-group-ingress --group-name wikipedia-minus-wikipedians-elasticache-$environment_name --protocol tcp --port 6379 --cidr 0.0.0.0/0

# Note: Cache cluster IDs are limited to 20 characters.
echo Creating ElastiCache cache cluster WMW$environment_name...
# TODO: take the cache node type from a command line argument
# TODO: pass an availability zone here
aws elasticache create-cache-cluster --cache-cluster-id WMW$environment_name --cache-node-type cache.t2.micro --engine redis --port 6379 --num-cache-nodes 1 --security-group-ids $(aws ec2 describe-security-groups --group-names wikipedia-minus-wikipedians-elasticache-$environment_name | ./parse_security_group_ids.py)

# Bring up EC2 instances
echo Creating EC2 security group wikipedia-minus-wikipedians-$environment_name...
aws ec2 create-security-group --group-name wikipedia-minus-wikipedians-$environment_name --description "Security group for EC2 instances in Wikipedia Minus Wikipedians environment $environment_name"
echo Authorizing inbound SSH for EC2 security group wikipedia-minus-wikipedians-$environment_name...
aws ec2 authorize-security-group-ingress --group-name wikipedia-minus-wikipedians-$environment_name --protocol tcp --port 22 --cidr 0.0.0.0/0
echo Authorizing inbound HTTP for EC2 security group wikipedia-minus-wikipedians-$environment_name...
aws ec2 authorize-security-group-ingress --group-name wikipedia-minus-wikipedians-$environment_name --protocol tcp --port 80 --cidr 0.0.0.0/0
# TODO: Fix the name of that key pair.
echo Bringing up $instances $instance_type EC2 instance\(s\)...
aws ec2 run-instances --image-id ami-d05e75b8 --key-name "Wikipedia Without Wikipedians" --user-data file://instance-setup.sh --count $instances --instance-type $instance_type --iam-instance-profile Name=Wikipedia-Minus-Wikipedians-$environment_name-EC2-Instance-Profile --security-groups wikipedia-minus-wikipedians-$environment_name > /tmp/run-instances-output.txt
for instance_id in $(cat /tmp/run-instances-output.txt | ./parse_instance_names.py)
do
    echo Tagging EC2 instance $instance_id with WikipediaMinusWikipediansEnvironment=$environment_name...
    aws ec2 create-tags --resources $instance_id --tags Key=WikipediaMinusWikipediansEnvironment,Value=$environment_name
done

echo Creating S3 bucket s3://Wikipedia-Minus-Wikipedians-Revisions-$environment_name...
aws s3 mb s3://Wikipedia-Minus-Wikipedians-Revisions-$environment_name --region us-east-1
cat s3_bucket_policy_template.json | sed s/ENVIRONMENT_NAME/$environment_name/ > /tmp/s3_bucket_policy.json
aws s3api put-bucket-policy --bucket Wikipedia-Minus-Wikipedians-Revisions-$environment_name --policy file:///tmp/s3_bucket_policy.json

for instance_id in $(cat /tmp/run-instances-output.txt | ./parse_instance_names.py)
do
    echo Waiting for instance $instance_id to be healthy...
    aws ec2 wait instance-status-ok --instance-ids $instance_id
done

echo Waiting for ElastiCache cache cluster WMW$environment_name to enter state \"available\"...
aws elasticache wait cache-cluster-available --cache-cluster-id WMW$environment_name
