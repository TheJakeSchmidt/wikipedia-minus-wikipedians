#!/bin/bash

# Destroys an AWS environment that was created by create_environment.sh. This should do the opposite
# of each action in that file, in reverse order.

echo Removing S3 bucket Wikipedia-Minus-Wikipedians-Revisions-QA...
aws s3 rm s3://Wikipedia-Minus-Wikipedians-Revisions-QA --recursive
aws s3 rb s3://Wikipedia-Minus-Wikipedians-Revisions-QA

for instance_id in $(aws ec2 describe-tags --filters Name=key,Values=WikipediaMinusWikipediansEnvironment Name=value,Values=QA Name=resource-type,Values=instance | ./parse_tags.py)
do
    echo Terminating EC2 instance $instance_id...
    aws ec2 terminate-instances --instance-ids $instance_id
done

# TODO: wait for instances to be terminated, then delete the security group.
#aws ec2 delete-security-group --group-name wikipedia-minus-wikipedians-QA

echo Deleting CodeDeploy deployment group WikipediaMinusWikipediansQA...
aws deploy delete-deployment-group --application-name WikipediaMinusWikipediansQA --deployment-group-name WikipediaMinusWikipediansQA
echo Deleting CodeDeploy application WikipediaMinusWikipediansQA...
aws deploy delete-application --application-name WikipediaMinusWikipediansQA

echo Detaching AWSCodeDeployRole policy from service role Wikipedia-Minus-Wikipedians-QA-CodeDeploy...
aws iam detach-role-policy --role-name Wikipedia-Minus-Wikipedians-QA-CodeDeploy --policy-arn arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole
echo Deleting service role Wikipedia-Minus-Wikipedians-QA-CodeDeploy...
aws iam delete-role --role-name Wikipedia-Minus-Wikipedians-QA-CodeDeploy

echo Removing role Wikipedia-Minus-Wikipedians-QA-EC2 from EC2 instance profile Wikipedia-Minus-Wikipedians-QA-EC2-Instance-Profile...
aws iam remove-role-from-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-QA-EC2-Instance-Profile --role-name Wikipedia-Minus-Wikipedians-QA-EC2
echo Deleting EC2 instance profile Wikipedia-Minus-Wikipedians-QA-EC2-Instance-Profile...
aws iam delete-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-QA-EC2-Instance-Profile
echo Deleting inline policy WikipediaMinusWikipediansQAEC2 from IAM role Wikipedia-Minus-Wikipedians-QA-EC2...
aws iam delete-role-policy --role-name Wikipedia-Minus-Wikipedians-QA-EC2 --policy-name WikipediaMinusWikipediansQAEC2
echo Deleting IAM role Wikipedia-Minus-Wikipedians-QA-EC2...
aws iam delete-role --role-name Wikipedia-Minus-Wikipedians-QA-EC2
