AWS_S3_BUCKET = 'my-accountabilitygadget-experiments-s3'

from os import error
from fabric import Connection, ThreadingGroup as Group
from fabric.exceptions import GroupException
from paramiko import RSAKey
from paramiko.ssh_exception import PasswordRequiredException, SSHException
from os.path import basename, splitext
from time import sleep
from math import ceil
from os.path import join
import subprocess

from benchmark.config import MyCommittee, Key, MyNodeParameters, MyBenchParameters, ConfigError
from benchmark.myutils import BenchError, Print, PathMaker, progress_bar
from benchmark.mycommands import CommandMaker
from benchmark.logs import LogParser, ParseError
from aws.instance import InstanceManager


def subprocess_run(*args, **kwargs):
    if 'stderr' in kwargs:
        del kwargs['stderr']
    print("Ok to run LOCALLY:", args, kwargs)
    # x = input()
    return subprocess.run(*args, **kwargs)


def g_run(g, cmd, *args, **kwargs):
    print("Running on REMOTE:", g)
    print("Cmd:", cmd)
    # x = input()
    return g.run(cmd, *args, **kwargs)


class FabricError(Exception):
    ''' Wrapper for Fabric exception with a meaningfull error message. '''

    def __init__(self, error):
        assert isinstance(error, GroupException)
        message = list(error.result.values())[-1]
        super().__init__(message)


class ExecutionError(Exception):
    pass


class Bench:
    def __init__(self, ctx):
        self.manager = InstanceManager.make()
        self.settings = self.manager.settings
        try:
            ctx.connect_kwargs.pkey = RSAKey.from_private_key_file(
                self.manager.settings.key_path
            )
            self.connect = ctx.connect_kwargs
        except (IOError, PasswordRequiredException, SSHException) as e:
            raise BenchError('Failed to load SSH key', e)

    def _check_stderr(self, output):
        if isinstance(output, dict):
            for x in output.values():
                if x.stderr:
                    raise ExecutionError(x.stderr)
        else:
            if output.stderr:
                raise ExecutionError(output.stderr)

    # def install(self):
    #     Print.info('Uploading SSH keys to access the repo...')
    #     hosts = self.manager.hosts(flat=True)
    #     g = Group(*hosts, user='ubuntu', connect_kwargs=self.connect)
    #     g.put('aws-experiment-nodes-id_rsa', remote='/home/ubuntu/.ssh/id_rsa')
    #     g.put('aws-experiment-nodes-id_rsa.pub', remote='/home/ubuntu/.ssh/id_rsa.pub')
    #     g.put('aws-experiment-nodes-known_hosts', remote='/home/ubuntu/.ssh/known_hosts')

    #     Print.info('Installing rust and cloning the repo, then rebooting...')
    #     cmd = [
    #         'sudo apt-get update',
    #         'sudo apt-get -y upgrade',
    #         'sudo apt-get -y autoremove',

    #         # To be able to access AWS S3 buckets
    #         'sudo apt-get -y awscli',

    #         # The following dependencies prevent the error: [error: linker `cc` not found].
    #         'sudo apt-get -y install build-essential cmake',

    #         # Some useful tools
    #         'sudo apt-get -y install net-tools ifstat vnstat zip unzip',

    #         # Install rust (non-interactive).
    #         'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y',
    #         'source $HOME/.cargo/env',
    #         'rustup default stable',

    #         # This is missing from the Rocksdb installer (needed for Rocksdb).
    #         'sudo apt-get install -y clang',

    #         # Clone the repo.
    #         f'(git clone {self.settings.repo_url} || (cd {self.settings.repo_name} ; git pull))',

    #         # Do a round of precompilation.
    #         # f'(cd {self.settings.repo_name} ; git pull ; cargo build --release --features benchmark)',
    #         f'(cd {self.settings.repo_name} ; git pull ; cargo build --release)',

    #         # Reboot.
    #         'sudo reboot',
    #     ]
    #     hosts = self.manager.hosts(flat=True)
    #     try:
    #         g = Group(*hosts, user='ubuntu', connect_kwargs=self.connect)
    #         g_run(g, ' && '.join(cmd), hide=False)
    #         Print.heading(f'Initialized testbed of {len(hosts)} nodes')
    #     except (GroupException, ExecutionError) as e:
    #         e = FabricError(e) if isinstance(e, GroupException) else e
    #         raise BenchError('Failed to install repo on testbed', e)

    def kill(self, hosts=[], delete_logs=False):
        assert isinstance(hosts, list)
        assert isinstance(delete_logs, bool)
        hosts = hosts if hosts else self.manager.hosts(flat=True)
        delete_logs = CommandMaker.clean_logs() if delete_logs else 'true'
        cmd = [delete_logs, f'({CommandMaker.kill()} || true)']
        try:
            g = Group(*list(set(hosts)), user='ubuntu', connect_kwargs=self.connect)
            g_run(g, ' && '.join(cmd), hide=False)
        except GroupException as e:
            raise BenchError('Failed to kill nodes', FabricError(e))

    def _select_hosts(self, bench_parameters):
        nodes = max(bench_parameters.nodes)

        # Ensure there are enough hosts.
        hosts = self.manager.hosts()
        if sum(len(x) for x in hosts.values()) < nodes:
            return []

        # Select the hosts in different data centers.
        ordered = zip(*hosts.values())
        ordered = [x for y in ordered for x in y]
        return ordered[:nodes]

    def _background_run(self, host, command, log_file, delay=0):
        name = splitext(basename(log_file))[0]
        if delay == 0:
            cmd = f'tmux new -d -s "{name}" "{command} |& tee {log_file}"'
        else:
            cmd = f'tmux new -d -s "{name}" "sleep {delay} ; {command} |& tee {log_file}"'
        c = Connection(host, user='ubuntu', connect_kwargs=self.connect)
        output = g_run(c, cmd, hide=False)
        self._check_stderr(output)

    def _update(self, hosts):
        Print.info(
            f'Updating {len(hosts)} nodes (branch "{self.settings.branch}")...'
        )
        cmd = [
            f'aws s3 cp s3://{self.settings.repo_url}/codecurrent.bundle .',
            f'(cd {self.settings.repo_name} && git fetch -f)',
            f'(cd {self.settings.repo_name} && git checkout -f {self.settings.branch})',
            f'(cd {self.settings.repo_name} && git pull -f)',
            'source $HOME/.cargo/env',
            f'(cd {self.settings.repo_name}/mynode && {CommandMaker.compile()})',
            CommandMaker.alias_binaries(
                f'./{self.settings.repo_name}/target/release/'
            )
        ]
        g = Group(*hosts, user='ubuntu', connect_kwargs=self.connect)
        g_run(g, ' && '.join(cmd), hide=False)

    def _config(self, hosts, node_parameters, num_faults):
        Print.info('Generating configuration files...')

        # Cleanup all local configuration files.
        cmd = CommandMaker.cleanup()
        subprocess_run([cmd], shell=True, stderr=subprocess.DEVNULL)

        # Recompile the latest code.
        cmd = CommandMaker.compile().split()
        subprocess_run(cmd, check=True, cwd=PathMaker.node_crate_path())

        # Create alias for the client and nodes binary.
        cmd = CommandMaker.alias_binaries(PathMaker.binary_path())
        subprocess_run([cmd], shell=True)

        # Generate configuration files.
        keys = []
        key_files = [PathMaker.key_file(i) for i in range(len(hosts))]
        for filename in key_files:
            cmd = CommandMaker.generate_key(filename).split()
            subprocess_run(cmd, check=True)
            keys += [Key.from_file(filename)]

        names = [x.name for x in keys]
        consensus_addr = [f'127.0.0.1:1234' for (i, x) in enumerate(hosts)]
        mempool_addr = [f'127.0.0.1:1234' for (i, x) in enumerate(hosts)]
        front_addr = [f'127.0.0.1:1234' for (i, x) in enumerate(hosts)]
        gossip_addr = [f'/ip4/{x}/tcp/{self.settings.gossip_port+i}' for (i, x) in enumerate(hosts)]
        committee = MyCommittee(num_faults, names, consensus_addr, mempool_addr, front_addr, gossip_addr)
        committee.print(PathMaker.committee_file())

        node_parameters.print(PathMaker.parameters_file())

        # prepare startup files
        from collections import defaultdict
        processes_to_start = defaultdict(set)
        progress = progress_bar(hosts, prefix='Preparing startup instructions:')

        for i, host in enumerate(progress):
            if i < len(hosts) - num_faults:
                processes_to_start[host].add(f'tmux new -d -s "node-{i}" "sleep 3 ; ./mynode -vv run --keys .node-{i}.json --committee .committee.json --store .db-{i} --parameters .parameters.json |& tee logs/node-{i}.log"')
            else:
                processes_to_start[host].add(f'tmux new -d -s "node-{i}" "sleep 3 ; ./mynode -vv run-adversary --keys .node-{i}.json --committee .committee.json --store .db-{i} --parameters .parameters.json |& tee logs/node-{i}.log"')

        progress = progress_bar(set(hosts), prefix='Writing startup instructions:')
        for i, host in enumerate(progress):
            f = open(f'.startup-{host}.sh', 'w')
            f.write('#! /bin/sh\n\n')
            f.write('\n'.join(processes_to_start[host]))
            f.write('\n')
            f.write(f'tmux new -d -s "monitor-traffic" "ifstat -n -t -i ens5 -T 0.5 |& tee logs/traffic-{host}.txt"\n')
            f.write(f'ifconfig > logs/traffic-total-before-me.txt\n')
            f.write('\n')
            f.close()


        # Cleanup all nodes.
        cmd = f'{CommandMaker.cleanup()} || true'
        g = Group(*list(set(hosts)), user='ubuntu', connect_kwargs=self.connect)
        g_run(g, cmd, hide=False)

        # # Upload configuration files.
        # progress = progress_bar(hosts, prefix='Uploading config files:')
        # for i, host in enumerate(progress):
        #     c = Connection(host, user='ubuntu', connect_kwargs=self.connect)
        #     c.put(PathMaker.committee_file(), '.')
        #     c.put(PathMaker.key_file(i), '.')
        #     c.put(PathMaker.parameters_file(), '.')

        # Prepare configuration files.
        from collections import defaultdict
        files_to_upload = defaultdict(set)
        progress = progress_bar(hosts, prefix='Preparing config files:')
        for i, host in enumerate(progress):
            files_to_upload[host].add(PathMaker.committee_file())
            files_to_upload[host].add(PathMaker.key_file(i))
            files_to_upload[host].add(PathMaker.parameters_file())

        print("Files to upload:", files_to_upload)

        # Compressing and uploading config files
        progress = progress_bar(set(hosts), prefix='Zipping and uploading config files:')
        for i, host in enumerate(progress):
            import zipfile
            zipf = zipfile.ZipFile(f'.config-{host}.zip', 'w', zipfile.ZIP_DEFLATED)
            for fn in files_to_upload[host]:
                # print("Host:", host, "File:", fn)
                zipf.write(fn)
            zipf.write(f'.startup-{host}.sh', arcname='.startup-me.sh')
            zipf.close()

            c = Connection(host, user='ubuntu', connect_kwargs=self.connect)
            c.put(f'.config-{host}.zip', '.config-me.zip')

        # decompressing config files
        cmd = f'unzip .config-me.zip'
        g = Group(*list(set(hosts)), user='ubuntu', connect_kwargs=self.connect)
        g_run(g, cmd, hide=False)

        return committee

    def _run_single(self, hosts, bench_parameters, node_parameters, debug=False):
        Print.info('Booting testbed...')

        # Kill any potentially unfinished run and delete logs.
        self.kill(hosts=hosts, delete_logs=True)

        # # Run the clients (they will wait for the nodes to be ready).
        # # Filter all faulty nodes from the client addresses (or they will wait
        # # for the faulty nodes to be online).
        # committee = Committee.load(PathMaker.committee_file())
        # addresses = committee.front_addresses()
        # addresses = [x for x in addresses if any(host in x for host in hosts)]
        # rate_share = ceil(rate / committee.size())
        # timeout = node_parameters.timeout_delay
        # client_logs = [PathMaker.client_log_file(i) for i in range(len(hosts))]
        # for host, addr, log_file in zip(hosts, addresses, client_logs):
        #     cmd = CommandMaker.run_client(
        #         addr,
        #         bench_parameters.tx_size,
        #         rate_share,
        #         timeout,
        #         nodes=addresses
        #     )
        #     self._background_run(host, cmd, log_file)

        # import time
        # now = int(time.time())
        # then = now + int(1.5 * len(hosts))   #30 #15 #30 #2*60

        # # Run the nodes.
        # key_files = [PathMaker.key_file(i) for i in range(len(hosts))]
        # dbs = [PathMaker.db_path(i) for i in range(len(hosts))]
        # node_logs = [PathMaker.node_log_file(i) for i in range(len(hosts))]
        # for host, key_file, db, log_file in zip(hosts, key_files, dbs, node_logs):
        #     cmd = CommandMaker.run_node(
        #         key_file,
        #         PathMaker.committee_file(),
        #         db,
        #         PathMaker.parameters_file(),
        #         debug=debug
        #     )
        #     self._background_run(host, cmd, log_file, delay=(then - int(time.time())))

        # import time
        # now = int(time.time())
        # then = now + int(3 * len(set(hosts)))   #30 #15 #30 #2*60

        # # Run the nodes.
        # key_files = [PathMaker.key_file(i) for i in range(len(hosts))]
        # dbs = [PathMaker.db_path(i) for i in range(len(hosts))]
        # node_logs = [PathMaker.node_log_file(i) for i in range(len(hosts))]
        # for host, key_file, db, log_file in zip(hosts, key_files, dbs, node_logs):
        #     cmd = CommandMaker.run_node(
        #         key_file,
        #         PathMaker.committee_file(),
        #         db,
        #         PathMaker.parameters_file(),
        #         debug=debug
        #     )
        #     self._background_run(host, cmd, log_file, delay=(then - int(time.time())))

        # Run the nodes.
        cmd = f'sh .startup-me.sh'
        g = Group(*list(set(hosts)), user='ubuntu', connect_kwargs=self.connect)
        g_run(g, cmd, hide=False)


        # Wait for the nodes to synchronize
        Print.info('Waiting for the nodes to synchronize...')
        sleep(2 * node_parameters.timeout_delay / 1000)

        # Wait for all transactions to be processed.
        duration = bench_parameters.duration
        for _ in progress_bar(range(20), prefix=f'Running benchmark ({duration} sec):'):
            sleep(ceil(duration / 20))

        self.kill(hosts=hosts, delete_logs=False)

    def _logs(self, hosts):
        # Delete local logs (if any).
        cmd = CommandMaker.clean_logs()
        subprocess_run([cmd], shell=True, stderr=subprocess.DEVNULL)

        # # Download log files.
        # progress = progress_bar(hosts, prefix='Downloading logs:')
        # for i, host in enumerate(progress):
        #     c = Connection(host, user='ubuntu', connect_kwargs=self.connect)
        #     c.get(PathMaker.node_log_file(i), local=PathMaker.node_log_file(i))
        #     # c.get(
        #     #     PathMaker.client_log_file(i), local=PathMaker.client_log_file(i)
        #     # )

        # # Prepare log download.
        # from collections import defaultdict
        # files_to_download = defaultdict(set)
        # progress = progress_bar(hosts, prefix='Preparing log download:')
        # for i, host in enumerate(progress):
        #     files_to_download[host].add((PathMaker.node_log_file(i), PathMaker.node_log_file(i)))

        # print("Files to download:", files_to_download)

        # # Download log files
        # progress = progress_bar(set(hosts), prefix='Downloading log files:')
        # for i, host in enumerate(progress):
        #     c = Connection(host, user='ubuntu', connect_kwargs=self.connect)
        #     for (fn_remote, fn_local) in files_to_download[host]:
        #         print("Host:", host, "File remote:", fn_remote, "File local:", fn_local)
        #         c.get(fn_remote, local=fn_local)

        def unique(lst):
            seen = []
            for l in lst:
                if not l in seen:
                    seen.append(l)
                    yield l

        # Compress logs on nodes
        # cmd = f'zip -9 -r .logs-me.zip logs/'
        cmd = f'ifconfig > logs/traffic-total-after-me.txt'
        g = Group(*list(unique(hosts)), user='ubuntu', connect_kwargs=self.connect)
        g_run(g, cmd, hide=False)
        cmd = f'cat logs/traffic-total-before-me.txt logs/traffic-total-after-me.txt > logs/traffic-total-me.txt'
        g = Group(*list(unique(hosts)), user='ubuntu', connect_kwargs=self.connect)
        g_run(g, cmd, hide=False)
        cmd = f'XZ_OPT=-9 tar -Jcvf .logs-me.tar.xz logs/'
        g = Group(*list(unique(hosts)), user='ubuntu', connect_kwargs=self.connect)
        g_run(g, cmd, hide=False)

        # # Download log files
        # progress = progress_bar(list(unique(hosts)), prefix='Downloading log files:')
        # for i, host in enumerate(progress):
        #     c = Connection(host, user='ubuntu', connect_kwargs=self.connect)
        #     # c.get('.logs-me.zip', local=f'.logs-{host}.zip')
        #     c.get('.logs-me.tar.xz', local=f'.logs-{host}.tar.xz')

        # Upload log files
        progress = progress_bar(list(unique(hosts)), prefix='Storing log files:')
        for i, host in enumerate(progress):
            print(i, host)
            c = Connection(host, user='ubuntu', connect_kwargs=self.connect)
            # c.get('.logs-me.zip', local=f'.logs-{host}.zip')
            # c.get('.logs-me.tar.xz', local=f'.logs-{host}.tar.xz')
            output = g_run(c, " && ".join([
                f"aws s3 cp .logs-me.tar.xz s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/.logs-{host}.tar.xz",
                f"aws s3 cp logs/traffic-total-me.txt s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/traffic-total-{host}.txt",
                f"aws s3 cp logs/traffic-total-before-me.txt s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/traffic-total-before-{host}.txt",
                f"aws s3 cp logs/traffic-total-after-me.txt s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/traffic-total-after-{host}.txt",
                f"aws s3 cp logs/traffic-{host}.txt s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/traffic-{host}.txt",
            ]), hide=False)
            self._check_stderr(output)
            # # output = g_run(c, f"aws s3 cp logs/traffic-total-after-me.txt s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/traffic-total-after-{host}.txt", hide=False)
            # # self._check_stderr(output)
            # output = g_run(c, , hide=False)
            # self._check_stderr(output)
            # output = g_run(c, , hide=False)
            # self._check_stderr(output)

        # if len(hosts) > 5:
        #     to_decompress = list(unique(hosts))[:2] + list(unique(hosts))[-2:]
        # else:
        #     to_decompress = list(unique(hosts))
        to_decompress = list(unique(hosts))[:1] + list(unique(hosts))[-1:]
        progress = progress_bar(to_decompress, prefix='Downloading and decompressing (sample of) log files:')
        for i, host in enumerate(progress):
            # subprocess_run([f'unzip .logs-{host}.zip'], shell=True, stderr=subprocess.DEVNULL)
            # subprocess_run([f'tar -Jxf .logs-{host}.tar.xz'], shell=True, stderr=subprocess.DEVNULL)
            subprocess_run([f"../venv/bin/aws s3 cp s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/.logs-{host}.tar.xz .logs-{host}.tar.xz"], shell=True, stderr=subprocess.DEVNULL)
            subprocess_run([f'tar -Jxf .logs-{host}.tar.xz'], shell=True, stderr=subprocess.DEVNULL)

        # # Parse logs and return the parser.
        # Print.info('Parsing logs and computing performance...')
        # return LogParser.process(PathMaker.logs_path(), faults=faults)

    def run(self, bench_parameters_dict, node_parameters_dict, debug=False):
        assert isinstance(debug, bool)
        Print.heading('Starting remote benchmark')
        try:
            bench_parameters = MyBenchParameters(bench_parameters_dict)
            node_parameters = MyNodeParameters(node_parameters_dict)
        except ConfigError as e:
            raise BenchError('Invalid nodes or bench parameters', e)

        # # Select which hosts to use.
        # selected_hosts = self._select_hosts(bench_parameters)
        # if not selected_hosts:
        #     Print.warn('There are not enough instances available')
        #     return
        selected_hosts = self.manager.hosts(flat=True)

        # Update nodes.
        self._update(selected_hosts)

        # Run benchmarks.
        assert len(bench_parameters.nodes) == 1
        n = bench_parameters.nodes[0]

        assert n % len(selected_hosts) == 0
        nodes_per_host = n // len(selected_hosts)

        Print.heading(f'\nRunning {n} nodes (nodes per host: {nodes_per_host})')
        from functools import reduce
        hosts = reduce(list.__add__, [ [h,]*nodes_per_host for h in selected_hosts ]) #[:n]

        # Upload all configuration files. (Handle adversary)
        num_faults = bench_parameters.faults
        self._config(hosts, node_parameters, num_faults)

        # Backup config
        print("Backup config")
        subprocess_run([f"fab info > machines_`sha256sum .committee.json | awk '{{ print $1; }}'`.txt"], shell=True, stderr=subprocess.DEVNULL)
        subprocess_run([f"../venv/bin/aws s3 cp .committee.json s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/"], shell=True, stderr=subprocess.DEVNULL)
        subprocess_run([f"../venv/bin/aws s3 cp fabfile.py s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/"], shell=True, stderr=subprocess.DEVNULL)
        subprocess_run([f"../venv/bin/aws s3 cp settings.json s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/"], shell=True, stderr=subprocess.DEVNULL)
        subprocess_run([f"../venv/bin/aws s3 cp machines_`sha256sum .committee.json | awk '{{ print $1; }}'`.txt s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/"], shell=True, stderr=subprocess.DEVNULL)
        for h in list(set(hosts)):
            print(h)
            subprocess_run([f"../venv/bin/aws s3 cp .config-{h}.zip s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/"], shell=True, stderr=subprocess.DEVNULL)
            subprocess_run([f"../venv/bin/aws s3 cp .startup-{h}.sh s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/"], shell=True, stderr=subprocess.DEVNULL)
        # subprocess_run([f"../venv/bin/aws s3 cp .committee.json s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/"], shell=True, stderr=subprocess.DEVNULL)
        # subprocess_run([f"../venv/bin/aws s3 cp .committee.json s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/"], shell=True, stderr=subprocess.DEVNULL)
        # subprocess_run([f"../venv/bin/aws s3 cp .committee.json s3://{AWS_S3_BUCKET}/experiment_`sha256sum .committee.json | awk '{{ print $1; }}'`/"], shell=True, stderr=subprocess.DEVNULL)

        # # Do not boot faulty nodes.
        # hosts = hosts[:n-faults]

        # Run the benchmark.
        assert bench_parameters.runs == 1
        i = bench_parameters.runs-1

        Print.heading(f'Run {i+1}/{bench_parameters.runs}')
        try:
            self._run_single(
                hosts, bench_parameters, node_parameters, debug
            )
            self._logs(hosts)
            # .print(PathMaker.result_file(
            #     n, r, bench_parameters.tx_size, faults
            # ))
        except (subprocess.SubprocessError, GroupException, ParseError) as e:
            self.kill(hosts=hosts)
            if isinstance(e, GroupException):
                e = FabricError(e)
            Print.error(BenchError('Benchmark failed', e))
            raise BenchError('Benchmark failed', e)


    def fetchlogs(self, bench_parameters_dict):
        Print.heading('Fetching logs')
        bench_parameters = MyBenchParameters(bench_parameters_dict)

        selected_hosts = self.manager.hosts(flat=True)
        n = bench_parameters.nodes[0]
        nodes_per_host = n // len(selected_hosts)

        from functools import reduce
        hosts = reduce(list.__add__, [ [h,]*nodes_per_host for h in selected_hosts ])

        self._logs(hosts)
