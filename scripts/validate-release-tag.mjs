import {appendFile} from 'node:fs/promises';
import {releaseImageTag} from './lib/release-tag.mjs';

const {imageTag, isStable} = releaseImageTag(process.env.RELEASE_TAG);
if (!process.env.GITHUB_OUTPUT) throw new Error('GITHUB_OUTPUT is required');
await appendFile(process.env.GITHUB_OUTPUT, `image_tag=${imageTag}\nis_stable=${isStable}\n`);
