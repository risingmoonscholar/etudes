"""The tools with a published release, and the one list the release gates read.

check-release.py and check-pages-release.py check these tools and no others. A
tool in the repository that is not listed here does not affect either gate.
Adding a tool to this list is the owner's step when its first release is
published; its README install line must exist by then.
"""
PUBLISHED_TOOLS = ("sweep", "stash", "unpack")
