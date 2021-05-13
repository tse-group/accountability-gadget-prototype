import subprocess
from math import ceil
from os.path import basename, join, splitext
from time import sleep

from benchmark.mycommands import CommandMaker
from benchmark.config import Key, MyLocalCommittee, MyNodeParameters, MyBenchParameters, ConfigError
from benchmark.logs import LogParser, ParseError
from benchmark.myutils import Print, BenchError, PathMaker


def subprocess_run(*args, **kwargs):
    if 'stderr' in kwargs:
        del kwargs['stderr']
    print("Ok to run:", args, kwargs)
    # x = input()
    return subprocess.run(*args, **kwargs)


class MyLocalBench:
    BASE_PORT = 7000

    def __init__(self, bench_parameters_dict, node_parameters_dict):
        try:
            self.bench_parameters = MyBenchParameters(bench_parameters_dict)
            self.node_parameters = MyNodeParameters(node_parameters_dict)
        except ConfigError as e:
            raise BenchError('Invalid nodes or bench parameters', e)

    def __getattr__(self, attr):
        return getattr(self.bench_parameters, attr)

    def _background_run(self, command, log_file):
        name = splitext(basename(log_file))[0]
        cmd = f'{command} 2> {log_file}'
        subprocess_run(['tmux', 'new', '-d', '-s', name, cmd], check=True)

    def _kill_nodes(self):
        try:
            cmd = CommandMaker.kill().split()
            subprocess_run(cmd, stderr=subprocess.DEVNULL)
        except subprocess.SubprocessError as e:
            raise BenchError('Failed to kill testbed', e)

    def run(self, debug=False):
        assert isinstance(debug, bool)
        Print.heading('Starting mylocal benchmark')

        # Kill any previous testbed.
        self._kill_nodes()

        try:
            Print.info('Setting up testbed...')
            nodes = self.nodes[0]

            # Cleanup all files.
            cmd = f'{CommandMaker.clean_logs()} ; {CommandMaker.cleanup()}'
            subprocess_run([cmd], shell=True, stderr=subprocess.DEVNULL)
            sleep(0.5) # Removing the store may take time.

            # Recompile the latest code.
            cmd = CommandMaker.compile().split()
            subprocess_run(cmd, check=True, cwd=PathMaker.node_crate_path())

            # Create alias for the client and nodes binary.
            cmd = CommandMaker.alias_binaries(PathMaker.binary_path())
            subprocess_run([cmd], shell=True)

            # Generate configuration files.
            keys = []
            key_files = [PathMaker.key_file(i) for i in range(nodes)]
            for filename in key_files:
                cmd = CommandMaker.generate_key(filename).split()
                subprocess_run(cmd, check=True)
                keys += [Key.from_file(filename)]

            names = [x.name for x in keys]
            committee = MyLocalCommittee(self.faults, names, self.BASE_PORT)
            committee.print(PathMaker.committee_file())

            self.node_parameters.print(PathMaker.parameters_file())

            # # Do not boot faulty nodes.
            # nodes = nodes - self.faults

            # Run the nodes.
            dbs = [PathMaker.db_path(i) for i in range(nodes)]
            node_logs = [PathMaker.node_log_file(i) for i in range(nodes)]
            for i_node, key_file, db, log_file in zip(range(nodes), key_files, dbs, node_logs):
                if i_node < nodes - self.faults:
                    cmd = CommandMaker.run_node(
                        key_file,
                        PathMaker.committee_file(),
                        db,
                        PathMaker.parameters_file(),
                        debug=debug
                    )
                else:
                    cmd = CommandMaker.run_adversarial_node(
                        key_file,
                        PathMaker.committee_file(),
                        db,
                        PathMaker.parameters_file(),
                        debug=debug
                    )
                self._background_run(cmd, log_file)

            # Wait for the nodes to synchronize
            Print.info('Waiting for the nodes to synchronize...')
            sleep(2 * self.node_parameters.timeout_delay / 1000)

            # Wait for all transactions to be processed.
            Print.info(f'Running benchmark ({self.duration} sec)...')
            sleep(self.duration)
            self._kill_nodes()

            # # Parse logs and return the parser.
            # Print.info('Parsing logs...')
            # return LogParser.process('./logs')

        except (subprocess.SubprocessError, ParseError) as e:
            self._kill_nodes()
            raise BenchError('Failed to run benchmark', e)
