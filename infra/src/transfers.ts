import * as aws from "@pulumi/aws";
import { tags } from "./config";

export function createTransferBucket(): aws.s3.Bucket {
  const bucket = new aws.s3.Bucket("transfer-bucket", {
    tags: {
      ...tags,
      Name: "HTTP Tunnel Ephemeral Transfers",
    },
  });

  new aws.s3.BucketPublicAccessBlock("transfer-bucket-public-access", {
    bucket: bucket.id,
    blockPublicAcls: true,
    blockPublicPolicy: true,
    ignorePublicAcls: true,
    restrictPublicBuckets: true,
  });

  new aws.s3.BucketServerSideEncryptionConfiguration("transfer-bucket-encryption", {
    bucket: bucket.id,
    rules: [{
      applyServerSideEncryptionByDefault: {
        sseAlgorithm: "AES256",
      },
    }],
  });

  new aws.s3.BucketLifecycleConfiguration("transfer-bucket-lifecycle", {
    bucket: bucket.id,
    rules: [{
      id: "expire-ephemeral-transfers",
      status: "Enabled",
      filter: { prefix: "transfers/" },
      expiration: { days: 1 },
      abortIncompleteMultipartUpload: { daysAfterInitiation: 1 },
    }],
  });

  return bucket;
}
