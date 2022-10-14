#! /usr/bin/env python

import click
import matplotlib.pyplot as plt
from datetime import timedelta, datetime
from tabulate import tabulate
import numpy as np
import os

from sfy.hub import Hub
from sfy.axl import Axl, AxlCollection
from sfy import signal
from sfy.timeutil import utcify
import logging

logger = logging.getLogger(__name__)


@click.group()
def axl():
    pass

@axl.command(name='list', help='List axl packages')
@click.argument('dev')
@click.option('--tx-start',
              default=None,
              help='Filter packages sent after this time',
              type=click.DateTime())
@click.option('--tx-end',
              default=None,
              help='Filter packages sent before this time',
              type=click.DateTime())
def list_buoys(dev, tx_start, tx_end):
    hub = Hub.from_env()
    buoy = hub.buoy(dev)
    logger.info(f"Listing packages for {buoy}")
    pcks = buoy.axl_packages_range(tx_start, tx_end)

    pcks = [[
        ax.start.strftime("%Y-%m-%d %H:%M:%S UTC"), ax.lon, ax.lat,
        ax.received_datetime.strftime("%Y-%m-%d %H:%M:%S UTC"),
        ax.storage_id, ax.fname
    ] for ax in pcks]
    print(
        tabulate(
            pcks,
            headers=['DataTime', 'Lon', 'Lat', 'TxTime', 'StID', 'File']))

@axl.command()
@click.argument('dev')
@click.option('--tx-start',
              default=None,
              help='Search in packages after this time (default: 24h ago)',
              type=click.DateTime())
@click.option('--tx-end',
              default=None,
              help='Search in packages before this time (default: now)',
              type=click.DateTime())
@click.option('--start',
              default=None,
              help='Clip results before this (default: tx-start)',
              type=click.DateTime())
@click.option('--end',
              default=None,
              help='Clip results after this (default: tx-end)',
              type=click.DateTime())
@click.option('--file',
              default=None,
              help='Store to this file',
              type=click.Path())
@click.option('--gap',
              default=None,
              help='Maximum gap allowed between packages before splitting into new segment (seconds).',
              type=float)
@click.option('--freq',
              default=None,
              help='Only use packages with this frequency (usually 52 or 20.8, within 2 Hz)',
              type=float)
def ts(dev, tx_start, tx_end, start, end, file, gap, freq):
    hub = Hub.from_env()
    buoy = hub.buoy(dev)

    if tx_start is None:
        tx_start = datetime.utcnow() - timedelta(days=1)

    if tx_end is None:
        tx_end = datetime.utcnow()

    if start is None:
        start = tx_start

    if end is None:
        end = tx_end

    if tx_start > start:
        tx_start = start

    if tx_end < end:
        tx_end = end

    tx_start = utcify(tx_start)
    tx_end = utcify(tx_end)
    start = utcify(start)
    end = utcify(end)

    logger.info(
        f"Scanning for packages tx: {tx_start} <-> {tx_end} and clipping between {start} <-> {end}"
    )

    pcks = buoy.axl_packages_range(tx_start, tx_end)
    logger.info(f"{len(pcks)} packages in tx range")

    if freq:
        pcks = list(filter(lambda p: abs(p.frequency - freq) <= 2, pcks))
        logger.info(f"Filtering packages on frequency: {freq}, {len(pcks)} packages matching.")

    pcks = AxlCollection(pcks)

    # filter packages between start and end
    pcks.clip(start, end)
    logger.info(f"{len(pcks)} in start <-> end range, splitting into segments..")

    gap = gap if gap is not None else AxlCollection.GAP_LIMIT

    segments = list(pcks.segments(eps_gap=gap))
    logger.info(f"Collection consists of: {len(segments)} segments")

    assert len(pcks) == sum(len(s) for s in segments)

    stable = [[
        s.start,
        s.end,
        s.duration,
        timedelta(seconds=s.duration),
        s.max_gap(),
        np.nan,
        len(s),
        s.pcks[0].storage_id,
        s.pcks[-1].storage_id,
    ] for s in segments]

    for i, _ in enumerate(stable[1:]):
        stable[i + 1][5] = (stable[i + 1][0] - stable[i][1])

    print(
        tabulate(stable,
                 headers=[
                     'Start', 'End', 'Duration (s)', 'Duration',
                     'Max Internal Gap', 'Segment Gap', 'Packages',
                     'Start ID', 'End ID',
                 ]))

    if file:
        logger.info(f"Saving to {file}..")

        assert not os.path.exists(file), "file exists"
        pcks.to_netcdf(file)

@axl.command(help='Plot package')
@click.argument('dev')
@click.argument('file')
def file(dev, file):
    hub = Hub.from_env()
    buoy = hub.buoy(dev)
    ax = buoy.package(file)

    a = signal.detrend(ax.z)
    _, _, w = signal.velocity(ax)
    _, _, u = signal.displacement(ax)
    u = signal.detrend(u)

    plt.figure()
    plt.title(
        f"Buoy: {buoy.dev}\n{ax.start} / {ax.received_datetime} length: {ax.duration}s f={ax.freq}Hz"
    )
    plt.plot(ax.time[:], a, label='acceleration ($m/s^2$)')
    plt.plot(ax.time[:-1], w, label='velocity ($m/s$)')
    plt.plot(ax.time[:-2], u, label='displacement ($m$)')

    print(ax.time[0])

    plt.grid()
    plt.legend()
    plt.xlabel('Time')
    plt.ylabel('Vertical movement $m$, $m/s$, $m/s^2$')

    plt.show()


@axl.command(help='Monitor buoy')
@click.argument('dev')
@click.option('--sleep',
              help='Time to sleep between update',
              default=5.0,
              type=float)
@click.option('--window', help='Time window to show (seconds).', default=60.0, type=float)
@click.option('--delay', help='Delay in data, use to re-play data in the past (seconds).', default=0.0, type=float)
def monitor(dev, sleep, window, delay):
    hub = Hub.from_env()
    buoy = hub.buoy(dev)


    plt.ion()
    fig = plt.figure()
    ax = fig.add_subplot(111)
    plt.grid()
    plt.legend()
    plt.xlabel('Time')
    plt.ylabel('Vertical movement $m$')

    la, = ax.plot([], [])

    while True:
        # Get packages from time-window
        end = datetime.utcnow() - timedelta(seconds=delay)
        start = end - timedelta(seconds=window)
        start, end = utcify(start), utcify(end)

        logger.info("Getting packages in window and up to now.")
        pcks = buoy.axl_packages_range(start - timedelta(minutes=20), None)
        if len(pcks) > 0:
            pcks = AxlCollection(pcks)
            logger.debug(f"{len(pcks)} packages in tx range")

            pcks.clip(start, end)
            logger.info(f"{len(pcks)} in start <-> end range")

            plt.title(
                f"Buoy: {buoy.dev}\n{pcks.start} -> {pcks.end} length: {pcks.duration}s f={pcks.frequency}Hz"
            )

            logger.debug("Integrating to displacement..")
            wz = signal.integrate(pcks.z, pcks.dt, order=2, method='dft')

            la.set_data(pcks.time[:-1], wz)

        fig.canvas.draw()
        fig.canvas.flush_events()

        if window is not None:
            plt.xlim([start, end])

        plt.pause(sleep)
