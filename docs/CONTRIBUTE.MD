# Discuss first
If you have a idea, make sure it is discussed before you make a PR. 

# Fmt And Clippy
You can use [just](https://github.com/casey/just) to help you fix your commit rapidly:
```shell
just fix
```

# How Can I Rewrite My Commit Message?
You can `git reset --soft upstream/main` and `git commit --forge`, this will merge your changes into one commit.

Or you also can use git rebase. But we will still merge them into one commit when it is merged.