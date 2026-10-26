# Static super receivers and module source prototypes — 2026-10-26

The 435-variant selection passes 434, gaining ten passes with zero losses
against the complete-report projection. Six gains are the earlier super key
coercion fix; two more are static super reads through a Proxy, and two are
host module-source objects inheriting from an intermediate host-type prototype.
The sole remaining failure is strictExecution.js script.

Host-type prototypes are shared within a realm, separated by host class name,
and isolated between realms. Core validation passes 386 library tests,
formatting, and all-target Clippy with warnings denied. The exact measured
source snapshot, worker identity, full selection, and comparison are retained.
Focused gains overlap earlier reports and must not be added twice. The latest
complete-corpus score remains 102,783 / 102,956.
