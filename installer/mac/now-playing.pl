#!/usr/bin/perl
# How Oyenza Studio runs MediaRemote Adapter's stream (crates/poni/src/mac/services/media.rs): the adapter's own
# script, with the same arguments, in this same process, so it reads macOS's Now Playing as /usr/bin/perl does.
#
#   /usr/bin/perl now-playing.pl <mediaremote-adapter.pl> <MediaRemoteAdapter.framework> stream [options]
#
# All this adds is an end: the app holds this process's stdin open and never writes to it, so reading it returns
# end-of-file only once the app has quit or crashed. A small child process waits for that, then sends the stream
# the SIGTERM that ends it (the adapter stops its run loop on SIGTERM). Without it, a stream whose app crashed
# would wait for the next song to notice. The child forks before the adapter loads anything.
use strict;
use warnings;

my $script = shift @ARGV;
die "usage: now-playing.pl <mediaremote-adapter.pl> <framework> <command> [options]\n" unless defined $script && -f $script;

my $stream = $$;
my $watcher = fork();
die "now-playing.pl: fork: $!\n" unless defined $watcher;
if ($watcher == 0) {
  my $buf;
  1 while sysread(STDIN, $buf, 4096);
  kill 'TERM', $stream;
  exit 0;
}
close STDIN;

do $script;
my $error = $@;
kill 'TERM', $watcher;
die $error if $error;
exit 0;
