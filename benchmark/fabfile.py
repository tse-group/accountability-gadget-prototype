AWS_S3_BUCKET = 'my-accountabilitygadget-experiments-s3'
NODES_PER_AWS_REGION = 5

LOCAL_bench_params = {
    'nodes': 8,
    'faults': 2, #0, #0, #2,
    'duration': 5*60, #600, #600, #300, #60, #120,
}
LOCAL_node_params = {
    'consensus': {
        'timeout_delay': 20_000,
        'sync_retry_delay': 100_000,
        'max_payload_size': 512_000,
        'min_block_delay': 5_000
    },
    'mempool': {
        'queue_capacity': 100_000,
        'sync_retry_delay': 100_000,
        'max_payload_size': 512_000,
        'min_block_delay': 0
    },
    'checkpoint': {
        'propose_k_deep': 6,
        'propose_inter_checkpoint_delay': 10_000,
        'propose_timeout': 60_000,
        'lcmine_slot': 7_500,
        'lcmine_slot_offset': 500,
        'lctree_logava_k_deep': 6,
    },
}

# LOCAL_bench_params = {
#     'nodes': 32,
#     'faults': 0, #0, #0, #2,
#     'duration': 30*60, #600, #600, #300, #60, #120,
# }
# LOCAL_node_params = {
#     'consensus': {
#         'timeout_delay': 20_000,
#         'sync_retry_delay': 100_000,
#         'max_payload_size': 512_000,
#         'min_block_delay': 5_000
#     },
#     'mempool': {
#         'queue_capacity': 100_000,
#         'sync_retry_delay': 100_000,
#         'max_payload_size': 512_000,
#         'min_block_delay': 0
#     },
#     'checkpoint': {
#         'propose_k_deep': 6,
#         'propose_inter_checkpoint_delay': 5*60_000,
#         'propose_timeout': 60_000,
#         'lcmine_slot': 7_500,
#         'lcmine_slot_offset': 500,
#         'lctree_logava_k_deep': 6,
#     },
# }

REMOTE_bench_params = {
    'nodes': 4100, #820, #4100, #82, #1_000,
    'faults': 0, #0, #0,
    'duration': 2500,#60*60, #120, #120, #30, #120,
}
REMOTE_node_params = {
    'consensus': {
        'timeout_delay': 20_000,
        'sync_retry_delay': 100_000,
        'max_payload_size': 512_000,
        'min_block_delay': 5_000
    },
    'mempool': {
        'queue_capacity': 100_000,
        'sync_retry_delay': 100_000,
        'max_payload_size': 512_000,
        'min_block_delay': 0
    },
    'checkpoint': {
        'propose_k_deep': 6,
        'propose_inter_checkpoint_delay': 5*60_000,
        'propose_timeout': 60_000,
        'lcmine_slot': 7_500,
        'lcmine_slot_offset': 500,
        'lctree_logava_k_deep': 6,
    },
}

from fabric import task

from benchmark.local import LocalBench
from benchmark.mylocal import MyLocalBench
from benchmark.logs import ParseError, LogParser
from benchmark.utils import Print
from benchmark.plot import Ploter, PlotError
from aws.instance import InstanceManager
from aws.remote import Bench, BenchError


@task
def mylocal(ctx):
    ''' Run benchmarks on localhost '''
    bench_params = LOCAL_bench_params
    node_params = LOCAL_node_params

    MyLocalBench(bench_params, node_params).run(debug=False) #.result()


@task
def create(ctx, nodes=NODES_PER_AWS_REGION):   # <--- nodes = number of nodes PER REGION!
    ''' Create a testbed'''
    deployinstall(ctx)
    InstanceManager.make().create_instances(nodes)


@task
def destroy(ctx):
    ''' Destroy the testbed '''
    InstanceManager.make().terminate_instances()


@task
def start(ctx):
    ''' Start all machines '''
    InstanceManager.make().start_instances()


@task
def stop(ctx):
    ''' Stop all machines '''
    InstanceManager.make().stop_instances()


@task
def info(ctx):
    ''' Display connect information about all the available machines '''
    InstanceManager.make().print_info()


# @task
# def install(ctx):
#     ''' Install HotStuff on all machines '''
#     Bench(ctx).install()


@task
def checkinstall(ctx):
    print("Root install script")
    ctx.run(f'../venv/bin/aws s3 ls s3://{AWS_S3_BUCKET}/ready/ | grep s1_ | wc -l')
    ctx.run(f'../venv/bin/aws s3 ls s3://{AWS_S3_BUCKET}/ready/ | grep s2_ | wc -l')
    ctx.run(f'../venv/bin/aws s3 ls s3://{AWS_S3_BUCKET}/ready/ | grep s3_ | wc -l')
    print("Ubuntu install script")
    ctx.run(f'../venv/bin/aws s3 ls s3://{AWS_S3_BUCKET}/ready/ | grep s4_ | wc -l')
    print("RUST INSTALL")
    ctx.run(f'../venv/bin/aws s3 ls s3://{AWS_S3_BUCKET}/ready/ | grep s5_ | wc -l')
    print("PRECOMPILE")
    ctx.run(f'../venv/bin/aws s3 ls s3://{AWS_S3_BUCKET}/ready/ | grep s6_ | wc -l')
    print("Unused")
    ctx.run(f'../venv/bin/aws s3 ls s3://{AWS_S3_BUCKET}/ready/ | grep s7_ | wc -l')
    ctx.run(f'../venv/bin/aws s3 ls s3://{AWS_S3_BUCKET}/ready/ | grep s8_ | wc -l')
    ctx.run(f'../venv/bin/aws s3 ls s3://{AWS_S3_BUCKET}/ready/ | grep s9_ | wc -l')


@task
def deployinstall(ctx):
    ctx.run(f'../venv/bin/aws s3 rm --recursive s3://{AWS_S3_BUCKET}/ready/')
    ctx.run('git bundle create codeinstall.bundle HEAD --branches --tags')
    ctx.run(f'../venv/bin/aws s3 cp install-root.sh s3://{AWS_S3_BUCKET}/')
    ctx.run(f'../venv/bin/aws s3 cp install-ubuntu.sh s3://{AWS_S3_BUCKET}/')
    ctx.run(f'../venv/bin/aws s3 cp codeinstall.bundle s3://{AWS_S3_BUCKET}/')
    deploycurrent(ctx)


@task
def deploycurrent(ctx):
    ctx.run('git bundle create codecurrent.bundle HEAD --branches --tags')
    ctx.run(f'../venv/bin/aws s3 cp codecurrent.bundle s3://{AWS_S3_BUCKET}/')


@task
def myremote(ctx):
    ''' Run benchmarks on AWS '''
    bench_params = REMOTE_bench_params
    node_params = REMOTE_node_params
    
    deploycurrent(ctx)
    Bench(ctx).run(bench_params, node_params, debug=False)


@task
def kill(ctx):
    ''' Stop any HotStuff execution on all machines '''
    Bench(ctx).kill()


@task
def fetchlogs(ctx):
    ''' Fetch log files '''
    bench_params = REMOTE_bench_params
    
    Bench(ctx).fetchlogs(bench_params)
