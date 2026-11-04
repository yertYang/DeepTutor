const numeric = '(0|[1-9]\\d*)';
const versionPattern = new RegExp(`^v${numeric}\\.${numeric}\\.${numeric}(?:(?:a|b|rc)${numeric})?(?:\\.post${numeric})?(?:\\.dev${numeric})?(?:\\+[0-9A-Za-z]+(?:[._-][0-9A-Za-z]+)*)?$`);
const stablePattern = new RegExp(`^v${numeric}\\.${numeric}\\.${numeric}$`);

export function releaseImageTag(tag) {
  if (typeof tag !== 'string' || versionPattern.exec(tag)?.[0] !== tag) {
    throw new Error('Release tag must be vX.Y.Z with an optional version suffix');
  }
  const imageTag = tag.slice(1).replaceAll('+', '-');
  if (imageTag.length > 128) throw new Error('Release image tag exceeds the OCI limit');
  return {imageTag, isStable: stablePattern.exec(tag)?.[0] === tag};
}
