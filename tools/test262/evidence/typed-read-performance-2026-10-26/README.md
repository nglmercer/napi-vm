# TypedArray element-read diagnostic — 2026-10-26

The candidate removes a temporary byte-vector allocation from ordinary
TypedArray element reads, but all four targeted large TypedArray staging
variants still time out with two workers and five-second deadlines. Its exact
source, worker identity, target report, and passing check log are retained;
the candidate was not retained in the working implementation.

A single-worker diagnostic of the overlapping-buffer test with source lengths
4,096, 16,384, and 65,536 finished in approximately 0.63, 2.46, and 9.43
seconds respectively under a 30-second diagnostic deadline. The first two
inputs were modified and are not corpus outcomes; the last is the original
size, still above the required five-second deadline. The roughly linear growth
indicates that sustained per-element work, rather than one allocation alone,
needs attention. The measured full run remains unchanged.
