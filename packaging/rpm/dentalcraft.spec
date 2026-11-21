# DentalCraft rpm, built by packaging/rpm/build.sh (sourced by packaging/installers.sh)
# from the release binaries, not from source: %install copies the tree staged by the
# script (macro stage). The binaries stay exactly as in the other packages: no strip,
# no debuginfo, no build-id links, no brp scripts.
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none
# xz payload: readable by every rpm >= 4.6, also outside Fedora.
%global _binary_payload w6.xzdio
%global _buildhost dentalcraft-release
%global use_source_date_epoch_as_buildtime 1
%global clamp_mtime_to_source_date_epoch 1

Name:           dentalcraft
Version:        %{pkg_version}
Release:        1
Summary:        Dental CAD for the practice and the lab
License:        PolyForm-Noncommercial-1.0.0
URL:            https://dentalcraft.app/
Vendor:         Daniele Viti
Packager:       DentalCraft Releases <releases@dentalcraft.app>
ExclusiveArch:  x86_64
# What the binaries link (glibc, libgcc) is found by rpm itself. The GUI libraries are
# loaded at run time: weak dependencies by soname, valid on any rpm distribution.
Recommends:     libxkbcommon.so.0()(64bit)
Recommends:     libwayland-client.so.0()(64bit)
Recommends:     libwayland-egl.so.1()(64bit)
Recommends:     libX11.so.6()(64bit)
Recommends:     libX11-xcb.so.1()(64bit)
Recommends:     libXcursor.so.1()(64bit)
Recommends:     libXi.so.6()(64bit)
Recommends:     libXrandr.so.2()(64bit)
Recommends:     libxkbcommon-x11.so.0()(64bit)
Recommends:     libvulkan.so.1()(64bit)
Recommends:     libEGL.so.1()(64bit)
Recommends:     mesa-vulkan-drivers
Recommends:     xdg-desktop-portal

%description
DentalCraft is an application for digital dentistry, starting with clear aligner
planning, from the intraoral scan to the print. It includes dentalcraft-cli, the
command line and MCP server that drive it headless or through a running app.

DentalCraft is not a medical device: do not use it on patients or for clinical
decisions. Source-available under the PolyForm Noncommercial License 1.0.0:
noncommercial use only.

The package also installs the DentalCraft dnf repository
(/etc/yum.repos.d/dentalcraft.repo), so that dnf keeps it up to date.

%install
cp -a %{stage}/. %{buildroot}/

%files
%{_bindir}/dentalcraft
%{_bindir}/dentalcraft-cli
%{_datadir}/applications/com.dentalcraft.DentalCraft.desktop
%{_datadir}/icons/hicolor/*/apps/com.dentalcraft.DentalCraft.*
%{_datadir}/metainfo/com.dentalcraft.DentalCraft.metainfo.xml
%dir %{_datadir}/dentalcraft
%{_datadir}/dentalcraft/install-kind
%config(noreplace) %{_sysconfdir}/yum.repos.d/dentalcraft.repo
%dir %{_docdir}/dentalcraft
%doc %{_docdir}/dentalcraft/README.md
%dir %{_licensedir}/dentalcraft
%license %{_licensedir}/dentalcraft/LICENSE.md
%license %{_licensedir}/dentalcraft/THIRD-PARTY-NOTICES.md
%license %{_licensedir}/dentalcraft/LICENSES
