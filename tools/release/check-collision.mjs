const version = process.argv[2];
if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version ?? "")) {
  throw new Error("Invalid launcher release version");
}
const repository = process.env.GITHUB_REPOSITORY;
const token = process.env.GH_TOKEN;
if (!/^[\w.-]+\/[\w.-]+$/.test(repository ?? "") || !token) {
  throw new Error("GitHub repository/token configuration is missing");
}
const tag = `v${version}`;
const response = await fetch(
  `https://api.github.com/repos/${repository}/releases/tags/${tag}`,
  {
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: "application/vnd.github+json",
      "User-Agent": "aurora-launcher-release-check",
    },
  },
);
if (response.status === 404) {
  console.log(`${tag}: no existing release`);
} else if (response.ok) {
  throw new Error(`${tag}: a release already exists`);
} else {
  throw new Error(`GitHub release collision check failed with HTTP ${response.status}`);
}
