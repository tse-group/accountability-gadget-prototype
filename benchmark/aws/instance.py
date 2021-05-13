import boto3
from botocore.exceptions import ClientError
from collections import defaultdict, OrderedDict
from time import sleep

from benchmark.utils import Print, BenchError, progress_bar
from aws.settings import Settings, SettingsError


class AWSError(Exception):
    def __init__(self, error):
        assert isinstance(error, ClientError)
        self.message = error.response['Error']['Message']
        self.code = error.response['Error']['Code']
        super().__init__(self.message)


class InstanceManager:
    INSTANCE_NAME = 'my-accountabilitygadget-experiments-node'
    SECURITY_GROUP_NAME = 'my-accountabilitygadget-experiments'

    def __init__(self, settings):
        assert isinstance(settings, Settings)
        self.settings = settings
        self.clients = OrderedDict()
        for region in settings.aws_regions:
            self.clients[region] = boto3.client('ec2', region_name=region)
            self.clients[region].my_region = region
            print(self.clients[region], self.clients[region].my_region)

    @classmethod
    def make(cls, settings_file='settings.json'):
        try:
            return cls(Settings.load(settings_file))
        except SettingsError as e:
            raise BenchError('Failed to load settings', e)

    def _get(self, state):
        # Possible states are: 'pending', 'running', 'shutting-down',
        # 'terminated', 'stopping', and 'stopped'.
        ids, ips = defaultdict(list), defaultdict(list)
        for region, client in self.clients.items():
            r = client.describe_instances(
                Filters=[
                    {
                        'Name': 'tag:Name',
                        'Values': [self.INSTANCE_NAME]
                    },
                    {
                        'Name': 'instance-state-name',
                        'Values': state
                    }
                ]
            )
            instances = [y for x in r['Reservations'] for y in x['Instances']]
            for x in instances:
                ids[region] += [x['InstanceId']]
                if 'PublicIpAddress' in x:
                    ips[region] += [x['PublicIpAddress']]
        return ids, ips

    def _wait(self, state):
        # Possible states are: 'pending', 'running', 'shutting-down',
        # 'terminated', 'stopping', and 'stopped'.
        while True:
            sleep(1)
            ids, _ = self._get(state)
            if sum(len(x) for x in ids.values()) == 0:
                break

    def _create_security_group(self, client):
        client.create_security_group(
            Description='My experiments with accountability gadgets',
            GroupName=self.SECURITY_GROUP_NAME,
        )

        client.authorize_security_group_ingress(
            GroupName=self.SECURITY_GROUP_NAME,
            IpPermissions=[
                {
                    'IpProtocol': 'tcp',
                    'FromPort': 22,
                    'ToPort': 22,
                    'IpRanges': [{
                        'CidrIp': '0.0.0.0/0',
                        'Description': 'Debug SSH access',
                    }],
                    'Ipv6Ranges': [{
                        'CidrIpv6': '::/0',
                        'Description': 'Debug SSH access',
                    }],
                },
                {
                    'IpProtocol': 'tcp',
                    'FromPort': self.settings.gossip_port,
                    'ToPort': self.settings.gossip_port + 5000,
                    'IpRanges': [{
                        'CidrIp': '0.0.0.0/0',
                        'Description': 'Ports for gossip communication of nodes (libp2p: gossipsub + kademlia)',
                    }],
                    'Ipv6Ranges': [{
                        'CidrIpv6': '::/0',
                        'Description': 'Ports for gossip communication of nodes (libp2p: gossipsub + kademlia)',
                    }],
                },
            ]
        )

    def _get_ami(self, client):
        # The AMI changes with regions.
        response = client.describe_images(
            Filters=[{
                'Name': 'description',
                'Values': ['Canonical, Ubuntu, 20.04 LTS, amd64 focal image build on 2020-10-26']
            }]
        )
        return response['Images'][0]['ImageId']

    def create_instances(self, instances):
        assert isinstance(instances, int) and instances > 0

        print("Create the security group in every region.")
        for client in self.clients.values():
            print(client, client.my_region)
            try:
                self._create_security_group(client)
            except ClientError as e:
                error = AWSError(e)
                if error.code != 'InvalidGroup.Duplicate':
                    raise BenchError('Failed to create security group', error)

        try:
            print("Create all instances.")
            size = instances * len(self.clients)
            # progress = progress_bar(
            #     self.clients.values(), prefix=f'Creating {size} instances'
            # )
            for client in self.clients.values():
                print(client, client.my_region)
                client.run_instances(
                    ImageId=self._get_ami(client),
                    InstanceType=self.settings.instance_type,
                    KeyName=self.settings.key_name,
                    MaxCount=instances,
                    MinCount=instances,
                    SecurityGroups=[self.SECURITY_GROUP_NAME],
                    TagSpecifications=[{
                        'ResourceType': 'instance',
                        'Tags': [{
                            'Key': 'Name',
                            'Value': self.INSTANCE_NAME
                        }]
                    }],
                    EbsOptimized=True,
                    BlockDeviceMappings=[{
                        'DeviceName': '/dev/sda1',
                        'Ebs': {
                            'VolumeType': 'gp2',
                            'VolumeSize': 50,
                            'DeleteOnTermination': True
                        }
                    }],
                    IamInstanceProfile={
                        'Name': 'FullAccessToS3BucketsForEc2Instances'
                    },
                    UserData=open('install-root.sh', 'r').read(),
                )

            # Wait for the instances to boot.
            Print.info('Waiting for all instances to boot...')
            self._wait(['pending'])
            Print.heading(f'Successfully created {size} new instances')
        except ClientError as e:
            raise BenchError('Failed to create AWS instances', AWSError(e))

    def terminate_instances(self):
        try:
            ids, _ = self._get(['pending', 'running', 'stopping', 'stopped'])
            size = sum(len(x) for x in ids.values())
            if size == 0:
                Print.heading(f'All instances are shut down')
                return

            print("Terminate instances.")
            for region, client in self.clients.items():
                print(client, client.my_region)
                if ids[region]:
                    client.terminate_instances(InstanceIds=ids[region])

            # Wait for all instances to properly shut down.
            Print.info('Waiting for all instances to shut down...')
            self._wait(['shutting-down'])

            print("Removing security groups.")
            for client in self.clients.values():
                print(client, client.my_region)
                client.delete_security_group(
                    GroupName=self.SECURITY_GROUP_NAME
                )

            Print.heading(f'Testbed of {size} instances destroyed')
        except ClientError as e:
            raise BenchError('Failed to terminate instances', AWSError(e))

    def start_instances(self):
        try:
            ids, _ = self._get(['stopping', 'stopped'])
            for region, client in self.clients.items():
                if ids[region]:
                    client.start_instances(InstanceIds=ids[region])
            size = sum(len(x) for x in ids.values())
            Print.heading(f'Starting {size} instances')
        except ClientError as e:
            raise BenchError('Failed to start instances', AWSError(e))

    def stop_instances(self):
        try:
            ids, _ = self._get(['pending', 'running'])
            for region, client in self.clients.items():
                if ids[region]:
                    client.stop_instances(InstanceIds=ids[region])
            size = sum(len(x) for x in ids.values())
            Print.heading(f'Stopping {size} instances')
        except ClientError as e:
            raise BenchError(AWSError(e))

    def hosts(self, flat=False):
        try:
            _, ips = self._get(['pending', 'running'])
            return [x for y in ips.values() for x in y] if flat else ips
        except ClientError as e:
            raise BenchError('Failed to gather instances IPs', AWSError(e))

    def hosts_with_ids(self):
        ids, ips = self._get(['pending', 'running'])
        return (ids, ips)

    def print_info(self):
        # hosts = self.hosts()
        (ids, hosts) = self.hosts_with_ids()
        key = self.settings.key_path
        text = ''
        for region, ips in hosts.items():
            text += f'\n Region: {region.upper()}\n'
            print(region, hosts[region], ids[region])
            assert(len(hosts[region]) == len(ids[region]))
            for i, ip in enumerate(ips):
                new_line = '\n' if (i+1) % 6 == 0 else ''
                text += f'{new_line} {i}\t{ids[region][i]}\tssh -i {key} -o "StrictHostKeyChecking no" ubuntu@{ip}\n'
        print(
            '\n'
            '----------------------------------------------------------------\n'
            ' INFO:\n'
            '----------------------------------------------------------------\n'
            f' Available machines: {sum(len(x) for x in hosts.values())}\n'
            f'{text}'
            '----------------------------------------------------------------\n'
        )
