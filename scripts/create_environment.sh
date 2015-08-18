#!/bin/bash

# Creates an AWS environment (QA, prod, test, etc.) for running and deploying Wikipedia Without
# Wikipedians. An environment consists of an EC2 instance profile (with an associated IAM role and
# policy), a service role for CodeDeploy, a COdeDeploy application and deployment group, a set of
# EC2 instances, and an S3 bucket to hold code revisions. The name of every resource created by this
# script contains the environment name, so that many environments can coexist in isolation from each
# other.

# TODO: Currently this script only creates an environment called "QA". Generalize it.
# TODO: bail out on error

# Create IAM instance profile
# Created based on the instructions at
# http://docs.aws.amazon.com/codedeploy/latest/userguide/how-to-create-iam-instance-profile.html.
echo Creating IAM role Wikipedia-Minus-Wikipedians-QA-EC2...
aws iam create-role --role-name Wikipedia-Minus-Wikipedians-QA-EC2 --assume-role-policy-document file://Wikipedia-Minus-Wikipedians-CodeDeploy-EC2-Trust.json
echo Attaching inline policy WikipediaMinusWikipediansQAEC2 to IAM role Wikipedia-Minus-Wikipedians-QA-EC2...
aws iam put-role-policy --role-name Wikipedia-Minus-Wikipedians-QA-EC2 --policy-name WikipediaMinusWikipediansQAEC2 --policy-document file://Wikipedia-Minus-Wikipedians-CodeDeploy-EC2-Permissions.json
echo Creating EC2 instance profile Wikipedia-Minus-Wikipedians-QA-EC2-Instance-Profile...
aws iam create-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-QA-EC2-Instance-Profile
echo Adding IAM role Wikipedia-Minus-Wikipedians-QA-EC2 to EC2 instance profile Wikipedia-Minus-Wikipedians-QA-EC2-Instance-Profile...
aws iam add-role-to-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-QA-EC2-Instance-Profile --role-name Wikipedia-Minus-Wikipedians-QA-EC2

# Create a service role for AWS CodeDeploy
echo Creating IAM service role Wikipedia-Minus-Wikipedians-QA-CodeDeploy...
aws iam create-role --role-name Wikipedia-Minus-Wikipedians-QA-CodeDeploy --assume-role-policy-document file://Wikipedia-Minus-Wikipedians-CodeDeploy-Trust.json
echo Attaching IAM policy AWSCodeDeployRole to service role Wikipedia-Minus-Wikipedians-QA-CodeDeploy...
aws iam attach-role-policy --role-name Wikipedia-Minus-Wikipedians-QA-CodeDeploy --policy-arn arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole

# There's a small delay before the new service role can be used to create a CodeDeploy deployment group.
echo Sleeping for 10 seconds...
sleep 10

# Create an AWS CodeDeploy application and deployment group
echo Creating CodeDeploy application WikipediaMinusWikipediansQA...
aws deploy create-application --application-name WikipediaMinusWikipediansQA
echo Creating CodeDeploy deployment group WikipediaMinusWikipediansQA...
aws deploy create-deployment-group --application-name WikipediaMinusWikipediansQA --deployment-group-name WikipediaMinusWikipediansQA --service-role-arn $(aws iam get-role --role-name Wikipedia-Minus-Wikipedians-QA-CodeDeploy --query "Role.Arn" --output text) --ec2-tag-filters Key=WikipediaMinusWikipediansEnvironment,Value=QA,Type=KEY_AND_VALUE

# Bring up EC2 instances
echo Creating EC2 security group wikipedia-minus-wikipedians-QA...
aws ec2 create-security-group --group-name wikipedia-minus-wikipedians-QA --description "Security group for Wikipedia Minus Wikipedians instance QA"
echo Authorizing inbound SSH for EC2 security group wikipedia-minus-wikipedians-QA...
aws ec2 authorize-security-group-ingress --group-name wikipedia-minus-wikipedians-QA --protocol tcp --port 22 --cidr 0.0.0.0/0
echo Authorizing inbound HTTP for EC2 security group wikipedia-minus-wikipedians-QA...
aws ec2 authorize-security-group-ingress --group-name wikipedia-minus-wikipedians-QA --protocol tcp --port 80 --cidr 0.0.0.0/0
# TODO: Fix the name of that key pair.
echo Bringing up 3 EC2 instances...
aws ec2 run-instances --image-id ami-d05e75b8 --key-name "Wikipedia Without Wikipedians" --user-data file://instance-setup.sh --count 3 --instance-type t2.micro --iam-instance-profile Name=Wikipedia-Minus-Wikipedians-QA-EC2-Instance-Profile --security-groups wikipedia-minus-wikipedians-QA > /tmp/run-instances-output.txt
for instance_id in $(cat /tmp/run-instances-output.txt | ./parse_instance_names.py)
do
    echo Tagging EC2 instance $instance_id with WikipediaMinusWikipediansEnvironment=QA...
    aws ec2 create-tags --resources $instance_id --tags Key=WikipediaMinusWikipediansEnvironment,Value=QA
done

echo Creating S3 bucket s3://Wikipedia-Minus-Wikipedians-Revisions-QA...
aws s3 mb s3://Wikipedia-Minus-Wikipedians-Revisions-QA --region us-east-1
aws s3api put-bucket-policy --bucket Wikipedia-Minus-Wikipedians-Revisions-QA --policy file://s3_bucket_policy.json

# TODO: wait for instances to be running/healthy?
