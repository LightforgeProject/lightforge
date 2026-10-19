# COPR's "make_srpm" build method: COPR clones the repo, runs
# `make -f .copr/Makefile srpm outdir=... spec=...` in a Fedora chroot, and
# builds whatever source RPM lands in $(outdir).
#
# The spec is always packaging/rpm/lightforge.spec; its Source0 is the tag's
# archive on GitHub, so this only fetches that and packs the two together.

SPEC := packaging/rpm/lightforge.spec

srpm:
	dnf -y install rpm-build rpmdevtools
	spectool --get-files --directory $(outdir) $(SPEC)
	rpmbuild -bs \
	  --define "_sourcedir $(outdir)" \
	  --define "_srcrpmdir $(outdir)" \
	  $(SPEC)
