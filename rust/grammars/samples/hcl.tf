# A small bucket.
variable "region" {
  type    = string
  default = "us-east-1"
}

resource "aws_s3_bucket" "files" {
  bucket = "myproject-${var.region}"
  tags = {
    Name  = "files"
    Count = 3
  }
}
