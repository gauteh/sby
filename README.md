[![sfy-data](https://github.com/gauteh/sfy/actions/workflows/sfy-data.yml/badge.svg)](https://github.com/gauteh/sfy/actions/workflows/sfy-data.yml)
[![sfy-buoy](https://github.com/gauteh/sfy/actions/workflows/sfy-buoy.yml/badge.svg)](https://github.com/gauteh/sfy/actions/workflows/sfy-buoy.yml)
[![sfy-processing](https://github.com/gauteh/sfy/actions/workflows/sfy-processing.yml/badge.svg)](https://github.com/gauteh/sfy/actions/workflows/sfy-processing.yml)

<img src="doc/logo/logo1.png" width="100px" />

# The small friendly buoy

* [sfy-buoy](sfy-buoy/) - the firmware for the wave buoy.
* [sfy-drifter](sfy-drifter/) - a minimal, modem & gps, only drifter buoy.
* [hardware](hardware/Hardware.md) | [build-tutorial](https://www.hackster.io/gaute-hope/ocean-buoy-to-measure-waves-drift-using-low-power-cellular-16ad09) | [bill-of-materials](https://docs.google.com/spreadsheets/d/e/2PACX-1vRE62P6-pCVzig-hSsqVcr2DABZ5LlB4lt1ZFfrct_tdcxoljO3zjmq7vGT1-jjqNiVCXLdns6XSkHF/pubhtml?gid=0&single=true) - hardware and assembly instructions.
* [sfy-data](sfy-data/) - the server scraping or receiving data from deployed
    buoys.
* [sfy-processing](sfy-processing/) - python libraries and tools for reading and post-processing received data.
* [sfy-dashboard](sfy-dashboard/) - web interface for displaying latest position
    and overview of buoys.

The buoys deployed in the surf on the coast of Norway:

[![Wave buoys in the surf zone at Jæren](http://img.youtube.com/vi/qK1Di7pjYFI/0.jpg)](http://www.youtube.com/watch?v=qK1Di7pjYFI "Wave buoys in the surf zone at Jæren")

For more information and attribution, please refer to:
* [Hope, Gaute, et al., 2024, (pre-print), SFY -- A lightweight, high-frequency and phase-resolving wave-buoy for coastal waters](https://arxiv.org/abs/2401.02286)
* [Seldal, Torunn Irene, M. Sc. thesis, 2023, _SFY: A free-drifting wave buoy for measuring surf waves_](https://bora.uib.no/bora-xmlui/handle/11250/3072146)

# Acknowledgements

This work is inspired by the [OpenMetBuoy-v2021a](https://github.com/jerabaul29/OpenMetBuoy-v2021a), see [Rabault et. al. (2022)](https://www.mdpi.com/2076-3263/12/3/110).
