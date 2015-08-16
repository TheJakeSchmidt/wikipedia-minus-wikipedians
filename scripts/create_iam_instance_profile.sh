#!/bin/bash

# Created based on the instructions at
# http://docs.aws.amazon.com/codedeploy/latest/userguide/how-to-create-iam-instance-profile.html. Not
# working or tested yet.

aws iam create-role --role-name Wikipedia-Minus-Wikipedians-EC2 --assume-role-policy-document file://Wikipedia-Minus-Wikipedians-CodeDeploy-EC2-Trust.json
aws iam put-role-policy --role-name Wikipedia-Minus-Wikipedians-EC2 --policy-name Wikipedia-Minus-Wikipedians-EC2-Permissions --policy-document file://Wikipedia-Minus-Wikipedians-CodeDeploy-EC2-Permissions.json
aws iam create-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-EC2-Instance-Profile
aws iam add-role-to-instance-profile --instance-profile-name Wikipedia-Minus-Wikipedians-EC2-Instance-Profile --role-name Wikipedia-Minus-Wikipedians-EC2
