# runner
a simple process runner

this is a simple process runner for my irc bot.
it reruns the specified process whenever it exits

## commands:
to stop (pause) the managed process:

``runner stop``

to start (resume) the managed process:

``runner start``

to force a restart of the managed process (sends a Ctrl-Break):

``runner restart``

to set the delay between each respawn:

``runner delay <secs>``

## configuration
configuration is done through environmental variables:

``RUNNER_PROCESS`` 
*defaults to noye.exe*

``RUNNER_PORT``
*defaults to 54145*

``RUNNER_LOG``
*defaults to info* 

see [env_logger](https://crates.io/crates/env_logger/)

## remote
the runner listens on ``localhost:54145`` by default, it accepts nul-terminated uppercase commands over tcp.

using the same form as the self commands:

``STOP\0``

``START\0``

``RESTART\0``

``RESPAWN 10\0``

## filesystem notification
the runner also listens for the managed process to be overwritten.

a restart command followed by the replacement of the executable will quickly start it, rather than waiting for the respawn delay.
