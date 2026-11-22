#!/bin/bash -e
install -m 644 files/linkmeterd.deb "${ROOTFS_DIR}/tmp/linkmeterd.deb"
on_chroot << CHROOT
apt-get update
apt-get install -y avrdude gpiod iw avahi-daemon network-manager fake-hwclock /tmp/linkmeterd.deb
rm /tmp/linkmeterd.deb
raspi-config nonint do_serial_hw 0
raspi-config nonint do_serial_cons 1
raspi-config nonint do_spi 0
systemctl enable linkmeterd
CHROOT
