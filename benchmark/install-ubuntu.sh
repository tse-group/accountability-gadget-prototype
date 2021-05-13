#! /bin/bash -ve

AWS_S3_BUCKET=my-accountabilitygadget-experiments-s3

MY_AMI_LAUNCH_INDEX=`curl -H "X-aws-ec2-metadata-token: $TOKEN" -v http://169.254.169.254/latest/meta-data/ami-launch-index`
MY_INSTANCE_ID=`curl -H "X-aws-ec2-metadata-token: $TOKEN" -v http://169.254.169.254/latest/meta-data/instance-id`

cd
touch /home/ubuntu/empty-file-as-flag

aws s3 cp /home/ubuntu/empty-file-as-flag s3://${AWS_S3_BUCKET}/ready/s4_${MY_INSTANCE_ID}


curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env
rustup default stable

aws s3 cp /home/ubuntu/empty-file-as-flag s3://${AWS_S3_BUCKET}/ready/s5_${MY_INSTANCE_ID}


aws s3 cp s3://${AWS_S3_BUCKET}/codeinstall.bundle .
aws s3 cp s3://${AWS_S3_BUCKET}/codecurrent.bundle .
git clone codeinstall.bundle accountability-gadget-prototype
cd accountability-gadget-prototype
git remote set-url origin /home/ubuntu/codecurrent.bundle
cargo build --release

aws s3 cp /home/ubuntu/empty-file-as-flag s3://${AWS_S3_BUCKET}/ready/s6_${MY_INSTANCE_ID}


sudo reboot
