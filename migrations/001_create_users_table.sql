-- 创建用户表
CREATE TABLE IF NOT EXISTS users (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id UUID NOT NULL UNIQUE,
    username VARCHAR(50) NOT NULL UNIQUE,
    email VARCHAR(100) NOT NULL,
    nickname VARCHAR(100),
    password_hash VARCHAR(255) NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- 创建索引,以email字段创建的索引
CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);
-- 创建索引,以username字段创建的索引
CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);
